"""Executable first-session scenarios; each receives a fresh project."""

import json
import os
import re
import shutil
import subprocess
import tempfile
from pathlib import Path

from tdd_cases import check_tdd_walkthrough

from tutorial_support import (
    EXAMPLE_DIR,
    PRACTICE_DIR,
    RULE_ID,
    BLOCK_SNIPPET,
    RULE_MESSAGE_FRAGMENT,
    DOCTOR_STATUSES,
    TRUST_FINDING_ID,
    LOG_EVIDENCE_ID,
    HOST_HOOKS_ID,
    PROBE_RULE_ID,
    PROBE_TEMP_PREFIX,
    FAILED,
    PASSED,
    SKIPPED,
    Result,
    run_command,
    hook_event,
    build_project,
    govern_project,
    snapshot,
    changed_paths,
    read_rules,
    load_log_entries,
    parse_doctor_report,
    finding_by_id,
    assert_trust_not_observed,
)


def check_fixtures(phr_mcp, project, env, result):
    required = [
        PRACTICE_DIR / ".phronesis" / "rules.json",
        PRACTICE_DIR / "src" / "visit.rs",
        PRACTICE_DIR / "notes.md",
    ]
    for path in required:
        if not path.is_file():
            result.fail(f"missing example fixture: {path}")

    rules_path = PRACTICE_DIR / ".phronesis" / "rules.json"
    if rules_path.is_file():
        try:
            rules = read_rules(rules_path)
        except json.JSONDecodeError as exc:
            result.fail(f"example rules.json is not valid JSON: {exc}")
            return
        entries = rules.get("rules", [])
        if len(entries) != 1:
            result.fail(
                f"expected exactly 1 deterministic rule, found {len(entries)}"
            )
            return
        rule = entries[0]
        if rule.get("id") != RULE_ID:
            result.fail(f"expected rule id {RULE_ID!r}, got {rule.get('id')!r}")
        if rule.get("phase") != "pre":
            result.fail("example rule must be phase 'pre' so the host blocks")
        predicates = [
            c.get("predicate") for c in rule.get("conditions", [])
        ]
        if predicates != ["new_content_contains"]:
            result.fail(
                "example rule conditions must be exactly one "
                f"new_content_contains predicate; found {predicates}"
            )
        pattern_args = [
            c.get("args", [None])[0]
            for c in rule.get("conditions", [])
            if c.get("predicate") == "new_content_contains"
        ]
        if not pattern_args or pattern_args[0] != BLOCK_SNIPPET:
            result.fail(
                "new_content_contains arg must be the literal "
                f"{BLOCK_SNIPPET!r}; found {pattern_args}"
            )
        actions = rule.get("actions", [])
        if not actions or actions[0].get("action_type") != "constraint_violation":
            result.fail("example rule must block via constraint_violation")


def check_missing_executable(phr_mcp, project, env, result):
    # Hooks invoke the bare command, even when this checker was given an
    # absolute candidate binary. Exercise absence instead of skipping it.
    empty_path = project / "empty-path"
    empty_path.mkdir()
    stripped_env = dict(env, PATH=str(empty_path))
    try:
        missing = run_command(["phr-mcp", "--version"], project, env=stripped_env)
    except FileNotFoundError:
        result.note("bare phr-mcp fails visibly when absent from the host PATH")
    except OSError as exc:
        result.fail(f"unexpected error testing missing executable: {exc}")
    else:
        result.fail(f"absent bare phr-mcp unexpectedly ran (exit {missing.returncode})")


def check_documented_setup(phr_mcp, project, env, result):
    """Execute the published setup blocks, not a parallel copy of them."""
    tutorial = EXAMPLE_DIR.parents[1] / "docs/tutorials/verify-installation.md"
    text = tutorial.read_text(encoding="utf-8")
    blocks = []
    for step in range(4):
        section = re.search(rf"(?ms)^## Step {step} .*?(?=^## |\Z)", text)
        block = re.search(r"```sh\n(.*?)```", section[0], re.S) if section else None
        if block is None:
            result.fail(f"tutorial Step {step} lacks its executable shell block")
            return
        blocks.append(block[1])
    # A miniature checkout prevents a missing cd in the instructions from
    # initializing the actual source tree. TMPDIR contains all scratch writes.
    checkout = project / "checkout"
    shutil.copytree(PRACTICE_DIR, checkout / "examples/first-governed-session/practice")
    script = "\n".join(blocks) + '\ntest -f "$SCRATCH/practice/.claude/settings.local.json"\n'
    binary = Path(shutil.which(phr_mcp)).resolve()
    child_env = dict(env, PATH=str(binary.parent) + os.pathsep + env.get("PATH", ""), TMPDIR=str(project))
    proc = run_command(["sh", "-eu", "-c", script], checkout, env=child_env)
    if proc.returncode != 0:
        result.fail(f"published setup commands exited {proc.returncode}: {proc.stdout[-800:]} {proc.stderr[-800:]}")
    if (checkout / ".claude").exists() or (checkout / ".phronesis").exists():
        result.fail("published setup initialized the checkout instead of the disposable copy")


def check_setup(phr_mcp, project, env, result):
    if not govern_project(phr_mcp, project, env, result):
        return

    settings_path = project / ".claude" / "settings.local.json"
    if not settings_path.is_file():
        result.fail("init did not write .claude/settings.local.json")
    else:
        try:
            settings = json.loads(settings_path.read_text(encoding="utf-8"))
            hooks = settings.get("hooks", {})
            pre = hooks.get("PreToolUse", []) if isinstance(hooks, dict) else []
            commands = [
                h.get("command")
                for entry in pre if isinstance(entry, dict)
                for h in entry.get("hooks", []) if isinstance(h, dict)
            ]
            if "phr-mcp pre-check" not in commands:
                result.fail(
                    "PreToolUse hooks do not include 'phr-mcp pre-check'; "
                    f"found {commands}"
                )
            post = hooks.get("PostToolUse", []) if isinstance(hooks, dict) else []
            post_commands = [
                h.get("command")
                for entry in post if isinstance(entry, dict)
                for h in entry.get("hooks", []) if isinstance(h, dict)
            ]
            if "phr-mcp post-check" not in post_commands:
                result.fail(
                    "PostToolUse hooks do not include 'phr-mcp post-check'; "
                    f"found {post_commands}"
                )
        except json.JSONDecodeError as exc:
            result.fail(
                f".claude/settings.local.json is not valid JSON: {exc}"
            )

    mcp_path = project / ".mcp.json"
    if not mcp_path.is_file():
        result.fail("init did not write .mcp.json")
    else:
        try:
            mcp = json.loads(mcp_path.read_text(encoding="utf-8"))
            servers = mcp.get("mcpServers", {})
            entry = servers.get("phronesis", {}) if isinstance(servers, dict) else {}
            if not isinstance(entry, dict) or entry.get("command") != "phr-mcp":
                result.fail(
                    ".mcp.json does not register the phronesis server with "
                    f"command 'phr-mcp'; found {entry}"
                )
        except json.JSONDecodeError as exc:
            result.fail(f".mcp.json is not valid JSON: {exc}")

    rules_path = project / ".phronesis" / "rules.json"
    if not rules_path.is_file():
        result.fail("shipped .phronesis/rules.json did not survive setup")
    else:
        try:
            if read_rules(rules_path) != read_rules(
                PRACTICE_DIR / ".phronesis" / "rules.json"
            ):
                result.fail(
                    "installed rules.json does not match the example's "
                    "deterministic rule set"
                )
        except json.JSONDecodeError as exc:
            result.fail(f"installed rules.json is not valid JSON: {exc}")


def assert_hook_outcome(phr_mcp, project, env, payload, expect_block, result, label):
    proc = run_command(
        [phr_mcp, "pre-check"],
        project,
        input_text=json.dumps(payload),
        env=env,
    )
    if expect_block:
        if proc.returncode != 2:
            result.fail(
                f"{label}: expected exit 2 (blocked), got {proc.returncode} "
                f"(stdout: {proc.stdout.strip()[:200]!r}, "
                f"stderr: {proc.stderr.strip()[:200]!r})"
            )
        combined = (proc.stdout or "") + (proc.stderr or "")
        if RULE_MESSAGE_FRAGMENT not in combined:
            result.note(
                f"{label}: block output did not name the rule "
                f"{RULE_ID!r}; host users may not see which rule fired"
            )
    else:
        if proc.returncode != 0:
            result.fail(
                f"{label}: expected exit 0 (allowed), got {proc.returncode} "
                f"(stdout: {proc.stdout.strip()[:200]!r}, "
                f"stderr: {proc.stderr.strip()[:200]!r})"
            )
        elif proc.stdout.strip() not in ("", "{}"):
            result.note(
                f"{label}: allow path printed {proc.stdout.strip()[:80]!r} "
                "(documented allow output is empty or {})"
            )


def check_allowed_blocked_hooks(phr_mcp, project, env, result):
    if not govern_project(phr_mcp, project, env, result):
        return
    visit = project / "src" / "visit.rs"
    notes = project / "notes.md"
    original_visit = visit.read_text(encoding="utf-8")

    assert_hook_outcome(
        phr_mcp, project, env,
        hook_event(
            "Edit", notes, "The governance setup works.\n",
            old_content="",
        ),
        False, result, "allowed Edit on notes.md",
    )
    assert_hook_outcome(
        phr_mcp, project, env,
        hook_event(
            "Write", notes, "# Practice notes\n\nA calm rewrite.\n",
        ),
        False, result, "allowed Write on notes.md",
    )
    assert_hook_outcome(
        phr_mcp, project, env,
        hook_event(
            "Edit", visit,
            original_visit + f"\n{BLOCK_SNIPPET}\n",
            old_content="",
        ),
        True, result, "blocked Edit on src/visit.rs",
    )
    assert_hook_outcome(
        phr_mcp, project, env,
        hook_event(
            "Write", notes, f"# Practice notes\n\n{BLOCK_SNIPPET}\n",
        ),
        True, result, "blocked Write on notes.md",
    )

    entries = load_log_entries(project)
    if entries is None:
        result.fail("no .phronesis/log.jsonl after hook runs; evidence trail missing")
        return
    pre_check = [e for e in entries if e.get("event") == "pre_check"]
    blocked = [e for e in pre_check if e.get("exit") == 2]
    if not blocked:
        result.fail(
            "log.jsonl records no exit-2 pre_check entry for the blocked edit"
        )
    else:
        reasons = [
            b
            for e in blocked
            for b in (e.get("blocked_by") or [])
            if b.get("rule") == RULE_ID
        ]
        if not reasons:
            result.note(
                f"blocked entries never name rule {RULE_ID!r} in blocked_by"
            )
    allowed = [e for e in pre_check if e.get("exit") == 0]
    if not allowed:
        result.note("log.jsonl records no exit-0 pre_check entry for allowed edits")


def fail_closed_case(phr_mcp, project, env, raw_rules, result, label):
    rules_path = project / ".phronesis" / "rules.json"
    rules_path.write_text(raw_rules, encoding="utf-8")
    proc = run_command(
        [phr_mcp, "pre-check"],
        project,
        input_text=json.dumps(
            hook_event("Write", project / "notes.md", "harmless\n")
        ),
        env=env,
    )
    if proc.returncode != 2:
        result.fail(
            f"{label}: expected fail-closed exit 2, got {proc.returncode} "
            f"(stdout: {proc.stdout.strip()[:200]!r}, "
            f"stderr: {proc.stderr.strip()[:200]!r})"
        )
        return
    combined = (proc.stdout or "") + (proc.stderr or "")
    if not combined.strip():
        result.fail(f"{label}: fail-closed exit 2 carried no explanatory output")
    else:
        result.note(
            f"{label}: blocked with: {combined.strip()[:160]!r}"
        )


def check_malformed_rules(phr_mcp, project, env, result):
    if not govern_project(phr_mcp, project, env, result):
        return
    fail_closed_case(
        phr_mcp, project, env,
        "{ this is not json\n",
        result, "malformed rules (invalid JSON)",
    )
    unknown_verb = {
        "rules": [
            {
                "id": "bogus-verb",
                "phase": "pre",
                "priority": 1,
                "conditions": [
                    {"predicate": "new_content_contains", "args": ["x"]}
                ],
                "actions": [
                    {"action_type": "delete_everything", "params": []}
                ],
            }
        ]
    }
    fail_closed_case(
        phr_mcp, project, env,
        json.dumps(unknown_verb),
        result, "malformed rules (unknown action verb)",
    )
    unknown_field = {
        "rules": [
            {
                "id": "bogus-field",
                "phase": "pre",
                "priority": 1,
                "bogus": 1,
                "conditions": [
                    {"predicate": "new_content_contains", "args": ["x"]}
                ],
                "actions": [
                    {"action_type": "constraint_violation", "params": ["m"]}
                ],
            }
        ]
    }
    fail_closed_case(
        phr_mcp, project, env,
        json.dumps(unknown_field),
        result, "malformed rules (unknown rule key)",
    )


def check_stale_config(phr_mcp, project, env, result):
    if not govern_project(phr_mcp, project, env, result):
        return
    wrong_case_phase = {
        "rules": [
            {
                "id": "stale-phase",
                "phase": "PRE",
                "priority": 1,
                "conditions": [
                    {"predicate": "new_content_contains", "args": ["x"]}
                ],
                "actions": [
                    {"action_type": "constraint_violation", "params": ["m"]}
                ],
            }
        ]
    }
    fail_closed_case(
        phr_mcp, project, env,
        json.dumps(wrong_case_phase),
        result, "stale config (case-wrong phase)",
    )
    duplicate_ids = {
        "rules": [
            {
                "id": "dup",
                "phase": "pre",
                "priority": 1,
                "conditions": [
                    {"predicate": "new_content_contains", "args": ["a"]}
                ],
                "actions": [
                    {"action_type": "constraint_violation", "params": ["m"]}
                ],
            },
            {
                "id": "dup",
                "phase": "pre",
                "priority": 2,
                "conditions": [
                    {"predicate": "new_content_contains", "args": ["b"]}
                ],
                "actions": [
                    {"action_type": "constraint_violation", "params": ["m"]}
                ],
            },
        ]
    }
    fail_closed_case(
        phr_mcp, project, env,
        json.dumps(duplicate_ids),
        result, "stale config (duplicate rule ids)",
    )


def doctor_argv(phr_mcp, project, with_json, probe=False):
    argv = [phr_mcp, "doctor", "--host", "claude", "--path", str(project)]
    if with_json:
        argv.append("--json")
    if probe:
        argv.append("--probe")
    return argv


def probe_temp_entries():
    return sorted(
        p.name
        for p in Path(tempfile.gettempdir()).iterdir()
        if p.name.startswith(PROBE_TEMP_PREFIX)
    )


def check_doctor_readonly(phr_mcp, project, env, result):
    if not govern_project(phr_mcp, project, env, result):
        return
    for with_json in (False, True):
        label = "doctor --json" if with_json else "doctor (plain)"
        before = snapshot(project)
        proc = run_command(doctor_argv(phr_mcp, project, with_json), project, env=env)
        after = snapshot(project)
        changes = changed_paths(before, after)
        if changes:
            result.fail(
                f"{label} is not read-only: {', '.join(changes[:8])}"
                + (f" (+{len(changes) - 8} more)" if len(changes) > 8 else "")
            )
        else:
            result.note(f"{label} left every project file untouched")
        if proc.returncode not in (0, 1):
            result.fail(
                f"{label} exited {proc.returncode} on a governed project; "
                "expected 0 (all ok/not_observed) or 1 (warn-only, e.g. "
                f"binary_on_path): stderr {(proc.stderr or '').strip()[:200]!r}"
            )
        if with_json:
            report = parse_doctor_report(proc, result, label)
            if report is not None:
                if report.get("probe") is not None:
                    result.fail(
                        f"{label}: probe is not null although --probe was "
                        "not passed"
                    )
                fails = [
                    f.get("id")
                    for f in report.get("findings", [])
                    if isinstance(f, dict) and f.get("status") == "fail"
                ]
                if fails:
                    result.fail(
                        f"{label}: governed project reported fail findings "
                        f"{fails}"
                    )
                assert_trust_not_observed(report, result, label)


def check_unobserved_trust(phr_mcp, project, env, result):
    if (project / ".claude" / "settings.local.json").exists():
        result.fail(
            "unobserved-trust check needs a project without host "
            "configuration; .claude/settings.local.json exists"
        )
    label = "doctor --json (no host configuration)"
    proc = run_command(doctor_argv(phr_mcp, project, True), project, env=env)
    if proc.returncode != 2:
        result.fail(
            f"{label}: expected exit 2 (host_hooks must fail), got "
            f"{proc.returncode}"
        )
    report = parse_doctor_report(proc, result, label)
    if report is None:
        return
    if report.get("probe") is not None:
        result.fail(f"{label}: probe is not null although --probe was not passed")
    trust = finding_by_id(report, TRUST_FINDING_ID)
    assert_trust_not_observed(report, result, label)
    log_finding = finding_by_id(report, LOG_EVIDENCE_ID)
    if log_finding is None:
        result.fail(f"{label}: no {LOG_EVIDENCE_ID} finding")
    elif log_finding.get("status") != "not_observed":
        result.fail(
            f"{label}: {LOG_EVIDENCE_ID} status is "
            f"{log_finding.get('status')!r} with no local log; expected "
            "not_observed"
        )
    hooks = finding_by_id(report, HOST_HOOKS_ID)
    if hooks is None:
        result.fail(f"{label}: no {HOST_HOOKS_ID} finding")
    elif hooks.get("status") != "fail":
        result.fail(
            f"{label}: {HOST_HOOKS_ID} status is {hooks.get('status')!r} "
            "with no hook configuration; expected fail"
        )
    plain = run_command(doctor_argv(phr_mcp, project, False), project, env=env)
    if plain.returncode != 2:
        result.fail(f"doctor (plain) without host configuration: expected exit 2, got {plain.returncode}")
    combined = ((plain.stdout or "") + (plain.stderr or "")).lower()
    if "not_observed" not in combined or TRUST_FINDING_ID not in combined:
        result.note(
            "doctor (plain) output does not visibly mark host_trust as "
            "not_observed; wording may have drifted"
        )


def check_log_evidence_not_proof(phr_mcp, project, env, result):
    if not govern_project(phr_mcp, project, env, result):
        return
    log_dir = project / ".phronesis"
    log_path = log_dir / "log.jsonl"
    stale = {
        "ts": 1000000000,
        "kind": "hook",
        "event": "pre_check",
        "phase": "pre",
        "tool": "Edit",
        "exit": 0,
        "consequences": [],
    }
    forged = {
        "ts": 1715717111,
        "kind": "lifecycle",
        "event": "host_observation",
        "note": "forged record claiming a host allowed an edit",
        "verdict": "allow",
    }
    with log_path.open("a", encoding="utf-8") as fh:
        fh.write(json.dumps(stale) + "\n")
        fh.write(json.dumps(forged) + "\n")
    proc = run_command(doctor_argv(phr_mcp, project, True), project, env=env)
    label = "doctor --json (forged + stale log evidence)"
    report = parse_doctor_report(proc, result, label)
    if report is None:
        return
    assert_trust_not_observed(report, result, label)
    log_finding = finding_by_id(report, LOG_EVIDENCE_ID)
    if log_finding is None:
        result.fail(f"{label}: no {LOG_EVIDENCE_ID} finding")
    elif log_finding.get("status") != "not_observed":
        result.fail(
            f"{label}: {LOG_EVIDENCE_ID} status is "
            f"{log_finding.get('status')!r} even though local log evidence "
            "is never proof; expected not_observed"
        )
    fails = [
        f.get("id")
        for f in report.get("findings", [])
        if isinstance(f, dict) and f.get("status") == "fail"
    ]
    if fails:
        result.fail(
            f"{label}: fabricated log records produced fail findings {fails}; "
            "forged evidence must never become a failure of configuration"
        )


def check_doctor_probe(phr_mcp, project, env, result):
    if not govern_project(phr_mcp, project, env, result):
        return
    before_entries = probe_temp_entries()
    before_project = snapshot(project)
    proc = run_command(
        doctor_argv(phr_mcp, project, True, probe=True), project, env=env
    )
    after_entries = probe_temp_entries()
    label = "doctor --probe --json"
    report = parse_doctor_report(proc, result, label)
    leftover = [
        name for name in after_entries if name not in before_entries
    ]
    if leftover:
        result.fail(
            f"{label}: probe fixture directories not removed: {leftover[:4]}"
        )
    if changed_paths(before_project, snapshot(project)):
        result.fail(f"{label}: probe touched the real project's files")
    if report is None:
        return
    probe = report.get("probe")
    if not isinstance(probe, dict):
        result.fail(f"{label}: probe is not an object although --probe was passed")
        return
    if probe.get("rule_id") != PROBE_RULE_ID:
        result.fail(
            f"{label}: probe rule_id is {probe.get('rule_id')!r}; expected "
            f"{PROBE_RULE_ID!r}"
        )
    if probe.get("ok") is not True:
        result.fail(f"{label}: probe.ok is {probe.get('ok')!r}; expected true")
    if probe.get("error") not in (None,):
        result.fail(f"{label}: probe.error is {probe.get('error')!r}; expected null")
    cases = probe.get("cases")
    if not isinstance(cases, list) or len(cases) != 2:
        result.fail(
            f"{label}: probe cases must be a list of 2 (blocked + allowed); "
            f"found {cases!r}"
        )
        return
    blocked_cases = [c for c in cases if isinstance(c, dict) and c.get("expect_block")]
    allowed_cases = [c for c in cases if isinstance(c, dict) and not c.get("expect_block")]
    if len(blocked_cases) != 1 or len(allowed_cases) != 1:
        result.fail(
            f"{label}: expected exactly one expect_block case and one allow "
            f"case; found {len(blocked_cases)}/{len(allowed_cases)}"
        )
        return
    blocked, allowed = blocked_cases[0], allowed_cases[0]
    if not blocked.get("matched_expectation") or blocked.get("exit_code") != 2:
        result.fail(
            f"{label}: blocked probe case did not match: "
            f"matched={blocked.get('matched_expectation')!r} "
            f"exit={blocked.get('exit_code')!r} decision={blocked.get('decision')!r}"
        )
    if not allowed.get("matched_expectation") or allowed.get("exit_code") != 0:
        result.fail(
            f"{label}: allowed probe case did not match: "
            f"matched={allowed.get('matched_expectation')!r} "
            f"exit={allowed.get('exit_code')!r} decision={allowed.get('decision')!r}"
        )
    if blocked.get("decision") != "block":
        result.fail(
            f"{label}: blocked probe decision is {blocked.get('decision')!r}; "
            "expected block"
        )
    if allowed.get("decision") != "allow":
        result.fail(
            f"{label}: allowed probe decision is {allowed.get('decision')!r}; "
            "expected allow"
        )
    if proc.returncode not in (0, 1):
        result.fail(
            f"{label}: exited {proc.returncode}; expected 0 or 1 when the "
            "probe matches and no finding fails"
        )
    plain = run_command(
        doctor_argv(phr_mcp, project, False, probe=True), project, env=env
    )
    plain_text = (plain.stdout or "") + (plain.stderr or "")
    if "probe" not in plain_text.lower() or PROBE_RULE_ID not in plain_text:
        result.note(
            "doctor --probe (plain) output does not visibly name the probe "
            "rule; wording may have drifted"
        )


def check_doctor_malformed_rules(phr_mcp, project, env, result):
    if not govern_project(phr_mcp, project, env, result):
        return
    (project / ".phronesis" / "rules.json").write_text(
        "{ this is not json\n", encoding="utf-8"
    )
    proc = run_command(doctor_argv(phr_mcp, project, True), project, env=env)
    label = "doctor --json (malformed rules)"
    report = parse_doctor_report(proc, result, label)
    if report is None:
        return
    if proc.returncode != 2:
        result.fail(
            f"{label}: expected exit 2 (a fail finding exists), got "
            f"{proc.returncode}"
        )
    fails = [
        f.get("id")
        for f in report.get("findings", [])
        if isinstance(f, dict) and f.get("status") == "fail"
    ]
    if not fails:
        result.fail(
            f"{label}: no fail finding although the rules file cannot load"
        )
    assert_trust_not_observed(report, result, label)


CHECKS = [
    ("tdd_walkthrough", check_tdd_walkthrough, "doctor"),
    ("fixtures", check_fixtures, "none"),
    ("missing_executable", check_missing_executable, "none"),
    ("documented_setup", check_documented_setup, "doctor"),
    ("setup", check_setup, "binary"),
    ("allowed_blocked_hooks", check_allowed_blocked_hooks, "binary"),
    ("malformed_rules", check_malformed_rules, "binary"),
    ("stale_config", check_stale_config, "binary"),
    ("doctor_readonly", check_doctor_readonly, "doctor"),
    ("unobserved_trust", check_unobserved_trust, "doctor"),
    ("log_evidence_not_proof", check_log_evidence_not_proof, "doctor"),
    ("doctor_probe", check_doctor_probe, "doctor"),
    ("doctor_malformed_rules", check_doctor_malformed_rules, "doctor"),
]
