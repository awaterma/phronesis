#!/usr/bin/env python3
"""Governance differential runner (stdlib only).

Runs an identical, isolated set of explicit fixture inputs against two
`phr-mcp` binaries (baseline vs candidate) and compares every observable:
exit codes, stdout, stderr, and on-disk state (.phronesis/rules.json,
log.jsonl, emitted-capsules.json). Raw artifacts are saved unmodified;
normalized copies (explicit nondeterminism only: the fixture temp project
root, named timestamp fields, the generated `s-YYYY-MM-DD-<hex>` session
id form) are compared. Rule/decision IDs, scope, error wording, and order
are compared verbatim.

Every case also carries positive expectations (exit codes, stderr needles,
JSONRPC success/error, required disk state transitions) so a run cannot
pass when both binaries return the same accidental error.

Usage:
  diff_runner.py --baseline-bin PATH --candidate-bin PATH --output-dir DIR \
      [--baseline-src DIR] [--candidate-src DIR]

Exit codes: 0 all cases matched and all assertions held;
1 at least one normalized mismatch; 3 a required case failed to run or was
omitted or an expectation failed; 2 operational error.
"""

import argparse
import difflib
import hashlib
import json
import os
import re
import selectors
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
FIXTURES = HERE / "fixtures"

REQUIRED_CASES = [
    "cli-help-version",
    "hook-pre-block-allow",
    "hook-post-warn",
    "hook-malformed-v2-verb",
    "hook-malformed-v1-arg",
    "hook-malformed-empty-when",
    "hook-malformed-duplicate-id",
    "hook-mixed-phases",
    "hook-silent-rule",
    "hook-or-clause",
    "hook-codex-decisions",
    "capsule-disk-output",
    "loader-layered-override",
    "loader-errors",
    "loader-optional-missing",
    "provider-reserved-rejection",
    "mcp-session-tools",
    "mcp-broken-load-refusal",
    "mcp-load-rules-file",
    "mcp-autopersist-write",
    "mcp-autoload",
    "mcp-no-autopersist",
    "mcp-capsule-emission",
]

RULES_MCP_CAPSULE_ADD = {
    "id": "mcp-capsule",
    "priority": 5,
    "conditions": [{"predicate": "status", "args": ["?state"]}],
    "actions": [{
        "action_type": "emit_capsule",
        "params": [],
        "data": {
            "id": "cap-from-mcp",
            "body": "state=?state",
            "lifecycle": "persistent",
            "priority": 50,
        },
    }],
}
RULES_ADD_V1 = {
    "id": "persisted-rule",
    "phase": "post",
    "priority": 10,
    "conditions": [{"predicate": "bash_command_matches", "args": ["git push --force"]}],
    "actions": [{"action_type": "constraint_warning", "params": ["force push seen"]}],
}
RULES_ADD_V2_SHAPED = {
    "id": "v2-shaped",
    "phase": "post",
    "priority": 10,
    "when": [{"new_content_contains": "x"}],
    "then": {"warn": "nope"},
}

CASES = [
    {
        "name": "cli-help-version",
        "rules": None,
        "calls": [
            {"argv": ["--help"], "expect": {"exit": 0, "stdout": ["Usage", "phr-mcp"]}},
            {"argv": ["--version"], "expect": {"exit": 0, "stdout": ["0."]}},
            {"argv": ["no-such-subcommand"], "expect": {"exit_not": 0}},
            {"argv": ["--help", "extra-arg"]},
        ],
    },
    {
        "name": "hook-pre-block-allow",
        "rules": "rules-pre-block.json",
        "calls": [
            {"argv": ["pre-check"], "payload": "payload-pre-force.json",
             "expect": {"exit": 2, "stderr": ["no force push"]}},
            {"argv": ["pre-check"], "payload": "payload-pre-clean.json",
             "expect": {"exit": 0, "stderr_not": ["no force push"]}},
        ],
        "expect_disk": [
            {"path": ".phronesis/rules.json", "contains": ["no-force-push"]},
            {"path": ".phronesis/log.jsonl", "absent": True},
        ],
    },
    {
        "name": "hook-post-warn",
        "rules": "rules-post-warn.json",
        "calls": [
            {"argv": ["post-check"], "payload": "payload-post-force.json",
             "expect": {"exit": 1, "stderr": ["force push seen"]}},
            {"argv": ["post-check"], "payload": "payload-post-clean.json",
             "expect": {"exit": 0}},
        ],
        "expect_disk": [
            {"path": ".phronesis/rules.json", "contains": ["force-push-seen"]},
            {"path": ".phronesis/log.jsonl", "absent": True},
        ],
    },
    {
        "name": "hook-malformed-v2-verb",
        "rules": "rules-bad-v2-verb.json",
        "calls": [
            {"argv": ["pre-check"], "payload": "payload-pre-force.json",
             "expect": {"exit": 2, "stderr": ["no-force-push", "then", "Block",
                                              "block, warn, log, emit_capsule"]}},
            {"argv": ["pre-check"], "payload": "payload-pre-clean.json",
             "expect": {"exit": 2, "stderr": ["no-force-push"]}},
            {"argv": ["post-check"], "payload": "payload-post-force.json",
             "expect": {"exit": 1, "stderr": ["no-force-push", "Block"]}},
            {"argv": ["codex-hook", "PreToolUse"], "payload": "payload-codex-deny.json",
             "expect": {"exit": 0, "stdout": ["permissionDecision\":\"deny\""]}},
        ],
    },
    {
        "name": "hook-malformed-v1-arg",
        "rules": "rules-bad-v1-arg.json",
        "calls": [{"argv": ["pre-check"], "payload": "payload-pre-force.json",
                   "expect": {"exit": 2, "stderr": ["no-force-push", "args", "string"]}}],
    },
    {
        "name": "hook-malformed-empty-when",
        "rules": "rules-bad-empty-when.json",
        "calls": [{"argv": ["pre-check"], "payload": "payload-pre-force.json",
                   "expect": {"exit": 2, "stderr": ["no-force-push", "when", "empty"]}}],
    },
    {
        "name": "hook-malformed-duplicate-id",
        "rules": "rules-bad-duplicate-id.json",
        "calls": [{"argv": ["pre-check"], "payload": "payload-pre-force.json",
                   "expect": {"exit": 2, "stderr": ["no-force-push", "duplicate"]}}],
    },
    {
        "name": "hook-mixed-phases",
        "rules": "rules-mixed-phases.json",
        "extra": {"push.sh": "git push --force\n"},
        "calls": [
            {"argv": ["pre-check"], "payload": "payload-pre-force.json",
             "expect": {"exit": 2, "stderr": ["no force push"]}},
            {"argv": ["pre-check"], "payload": "payload-pre-clean.json",
             "expect": {"exit": 0}},
            {"argv": ["post-check"], "payload": "payload-post-force.json",
             "expect": {"exit": 1, "stderr": ["force push seen"]}},
            {"argv": ["post-check"], "payload": "payload-post-clean.json",
             "expect": {"exit": 0}},
            {"argv": ["audit", "--json"],
             "expect": {"exit": 0, "stdout": ["r-audit"]}},
        ],
        "expect_disk": [
            {"path": ".phronesis/rules.json",
             "contains": ["r-pre", "r-post", "r-audit", "r-none"]},
        ],
    },
    {
        "name": "hook-silent-rule",
        "rules": "rules-silent.json",
        "calls": [
            {"argv": ["pre-check"], "payload": "payload-pre-force.json",
             "expect": {"exit": 2}},
            {"argv": ["pre-check"], "payload": "payload-pre-clean.json",
             "expect": {"exit": 0}},
        ],
        "expect_disk": [
            {"path": ".phronesis/rules.json", "contains": ["silent-block"]},
        ],
    },
    {
        "name": "hook-or-clause",
        "rules": "rules-or.json",
        "calls": [
            {"argv": ["post-check"], "payload": "payload-post-drop.json",
             "expect": {"exit": 1, "stderr": ["destructive command"]}},
            {"argv": ["post-check"], "payload": "payload-post-rmrf.json",
             "expect": {"exit": 1}},
            {"argv": ["post-check"], "payload": "payload-post-clean.json",
             "expect": {"exit": 0}},
        ],
    },
    {
        "name": "hook-codex-decisions",
        "rules": "rules-codex.json",
        "calls": [
            {"argv": ["codex-hook", "PreToolUse"], "payload": "payload-codex-clean.json",
             "expect": {"exit": 0, "stdout": ["{}"], "stdout_not": ["permissionDecision"]}},
            {"argv": ["codex-hook", "PreToolUse"], "payload": "payload-codex-warn.json",
             "expect": {"exit": 0, "stdout": ["additionalContext\":\"deploy needs review",
                                              "hookEventName\":\"PreToolUse"],
                        "stdout_not": ["permissionDecision"]}},
            {"argv": ["codex-hook", "PreToolUse"], "payload": "payload-codex-deny.json",
             "expect": {"exit": 0, "stdout": ["permissionDecision\":\"deny",
                                              "no force push"]}},
            {"argv": ["codex-hook", "UserPromptSubmit"], "payload": "payload-codex-prompt.json",
             "expect": {"exit": 0, "stdout": ["{}"]}},
        ],
    },
    {
        "name": "capsule-disk-output",
        "rules": "rules-capsule-or.json",
        "calls": [
            {"argv": ["post-check"], "payload": "payload-post-drop.json",
             "expect": {"exit": 0, "stderr_not": ["any"]}},
            {"argv": ["context", "list", "--json", "--path", "<ROOT>"],
             "expect": {"exit": 0, "stdout": ["cap-danger", "or1"]}},
        ],
        "expect_disk": [
            {"path": ".phronesis/emitted-capsules.json",
             "contains": ["cap-danger", "cap-danger#or1", "session"]},
        ],
    },
    {
        "name": "loader-layered-override",
        "rules": None,
        "loader": {"template": "loader-template.json",
                   "base": "loader-base-rules.json",
                   "override": "loader-override-rules.json"},
        "no_action_log": False,
        "calls": [
            {"argv": ["pre-check"], "payload": "payload-pre-force.json",
             "expect": {"exit": 0, "stderr_not": ["base layer", "any"]}},
            {"argv": ["pre-check"], "payload": "payload-pre-clean.json",
             "expect": {"exit": 0}},
        ],
        "expect_disk": [
            {"path": ".phronesis/log.jsonl",
             "contains": ["overridden: force push is logged by the override layer",
                          "\"action_type\":\"log\""]},
        ],
    },
    {
        "name": "loader-errors",
        "rules": None,
        "variants": [
            {"loader": {"template": "loader-template.json",
                        "base": "loader-base-rules.json",
                        "override": "loader-override-rules.json",
                        "extra_layers": [{"name": "ghost",
                                          "path": "/nonexistent/compat/ghost-rules.json"}]},
             "suffix": "missing-layer",
             "calls": [{"argv": ["pre-check"], "payload": "payload-pre-clean.json",
                        "expect": {"exit": 2, "stderr": ["ghost", "does not exist"]}}]},
            {"loader": {"inline": {"version": 2,
                                   "layers": [{"name": "base", "path": "rules.json"}]}},
             "suffix": "version-2",
             "calls": [{"argv": ["pre-check"], "payload": "payload-pre-clean.json",
                        "expect": {"exit": 2, "stderr": ["version 2", "expected 1"]}}]},
        ],
        "calls": [],
    },
    {
        "name": "loader-optional-missing",
        "rules": None,
        "loader": {"template": "loader-template.json",
                   "base": "loader-base-rules.json",
                   "override": "loader-override-rules.json",
                   "optional_override": True,
                   "delete_override": True},
        "calls": [{"argv": ["pre-check"], "payload": "payload-pre-force.json",
                   "expect": {"exit": 2, "stderr": ["base layer"]}}],
    },
    {
        "name": "provider-reserved-rejection",
        "rules": "rules-provider-reserved.json",
        "providers": {"forge.rhai": "providers/forge.rhai"},
        "calls": [{"argv": ["pre-check"], "payload": "payload-edit.md",
                   "expect": {"exit": 2, "stderr": ["forge.rhai", "reserved",
                                                    "signal_pass"]}}],
    },
    {
        "name": "mcp-session-tools",
        "rules": None,
        "mcp": [
            {"method": "tools/list", "params": {},
             "expect": {"contains": ["add_rule", "list_rules", "fire_rules", "save_rules",
                                     "remove_rule", "load_rules_file"]}},
            {"tool": "add_rule", "args": RULES_ADD_V1, "expect": {"ok": True},
             "expect_disk": {"path": ".phronesis/rules.json",
                             "contains": ["persisted-rule"]}},
            {"tool": "add_rule", "args": RULES_ADD_V2_SHAPED,
             "expect": {"error": True, "contains": ["missing field `conditions`"]}},
            {"tool": "list_rules", "args": {},
             "expect": {"contains": ["persisted-rule", "bash_command_matches"]}},
            {"tool": "assert_fact",
             "args": {"id": "cmd-force", "predicate": "bash_command_matches",
                      "args": ["git push --force"]},
             "expect": {"ok": True}},
            {"tool": "fire_rules", "args": {},
             "expect": {"ok": True, "contains": ["\"actions_fired\": 1", "persisted-rule"]}},
            {"tool": "remove_rule", "args": {"rule_id": "persisted-rule"},
             "expect": {"ok": True}},
            {"tool": "list_rules", "args": {}, "expect": {"not_contains": ["persisted-rule"]}},
        ],
        "expect_disk": [
            {"path": ".phronesis/rules.json", "not_contains": ["persisted-rule"]},
        ],
    },
    {
        "name": "mcp-broken-load-refusal",
        "rules": "rules-broken.json",
        "mcp": [
            {"tool": "list_rules", "args": {},
             "expect": {"contains": ["load_error", "broken-rule", "empty"]}},
            {"tool": "add_rule", "args": RULES_ADD_V1,
             "expect": {"error": True, "contains": ["refusing to change rules",
                                                    "partial set"]}},
            {"write_disk": {"path": ".phronesis/rules.json", "fixture": "rules-pre-block.json"},
             "out": "ok"},
            {"tool": "add_rule", "args": RULES_ADD_V1, "expect": {"ok": True}},
            {"tool": "list_rules", "args": {},
             "expect": {"contains": ["persisted-rule"], "not_contains": ["load_error"]}},
        ],
        "expect_disk": [
            {"path": ".phronesis/rules.json", "contains": ["persisted-rule", "no-force-push"]},
        ],
    },
    {
        "name": "mcp-load-rules-file",
        "rules": None,
        "mcp": [
            {"tool": "list_rules", "args": {}, "expect": {"not_contains": ["external-rule"]}},
            {"write_disk": {"path": ".phronesis/rules.json", "fixture": "rules-external.json"},
             "out": "ok"},
            {"tool": "load_rules_file", "args": {},
             "expect": {"ok": True}},
            {"tool": "list_rules", "args": {},
             "expect": {"contains": ["external-rule"]}},
        ],
        "expect_disk": [
            {"path": ".phronesis/rules.json", "contains": ["external-rule"]},
        ],
    },
    {
        "name": "mcp-autopersist-write",
        "rules": None,
        "mcp": [
            {"tool": "add_rule", "args": RULES_ADD_V1, "expect": {"ok": True},
             "expect_disk": {"path": ".phronesis/rules.json",
                             "contains": ["persisted-rule"]}},
            {"tool": "save_rules", "args": {"dry_run": True},
             "expect": {"ok": True, "contains": ["dry_run"]},
             "expect_disk": {"path": ".phronesis/rules.json",
                             "contains": ["persisted-rule"]}},
        ],
        "expect_disk": [
            {"path": ".phronesis/rules.json", "contains": ["persisted-rule"]},
        ],
    },
    {
        "name": "mcp-autoload",
        "rules": "rules-autoload.json",
        "mcp": [
            {"tool": "list_rules", "args": {},
             "expect": {"contains": ["persisted-rule"]}},
        ],
        "calls_after": [{"argv": ["post-check"], "payload": "payload-post-force.json",
                         "expect": {"exit": 1, "stderr": ["force push seen"]}}],
        "expect_disk": [
            {"path": ".phronesis/rules.json", "contains": ["persisted-rule"]},
        ],
    },
    {
        "name": "mcp-no-autopersist",
        "rules": None,
        "env": {"PHRONESIS_NO_AUTOPERSIST": "1"},
        "mcp": [
            {"tool": "add_rule", "args": RULES_ADD_V1, "expect": {"ok": True},
             "expect_disk": {"path": ".phronesis/rules.json", "absent": True}},
            {"tool": "save_rules", "args": {"dry_run": True}, "expect": {"ok": True},
             "expect_disk": {"path": ".phronesis/rules.json", "absent": True}},
            {"tool": "save_rules", "args": {}, "expect": {"ok": True},
             "expect_disk": {"path": ".phronesis/rules.json",
                             "contains": ["persisted-rule"]}},
        ],
        "expect_disk": [
            {"path": ".phronesis/rules.json", "contains": ["persisted-rule"]},
        ],
    },
    {
        "name": "mcp-capsule-emission",
        "rules": None,
        "env": {"PHRONESIS_NO_AUTOPERSIST": "1"},
        "mcp": [
            {"tool": "add_rule", "args": RULES_MCP_CAPSULE_ADD, "expect": {"ok": True}},
            {"tool": "assert_fact",
             "args": {"id": "status-ready", "predicate": "status", "args": ["ready"]},
             "expect": {"ok": True}},
            {"tool": "fire_rules", "args": {},
             "expect": {"ok": True, "contains": ["\"actions_fired\": 1", "mcp-capsule",
                                                 "state=ready"]}},
            {"tool": "list_emitted_capsules", "args": {},
             "expect": {"ok": True, "contains": ["cap-from-mcp", "persistent"]}},
            {"tool": "retract_emitted_capsule", "args": {"id": "cap-from-mcp"},
             "expect": {"ok": True, "contains": ["\"removed\": true"]}},
            {"tool": "retract_emitted_capsule", "args": {"id": "cap-from-mcp"},
             "expect": {"ok": True, "contains": ["\"removed\": false", "\"success\": true"]}},
        ],
        "expect_disk": [],
    },
]

# Narrow normalization: only identified generated nondeterminism.
NAMED_TS = re.compile(
    r'(\\?"(?:ts|ts_ms|generated_at|scan_duration_ms|created_at_ms|updated_at_ms|expires_at|emitted_at)\\?"\s*:\s*)-?\d+(?:\.\d+)?')
SESSION_ID = re.compile(
    r'(\\?"session_id\\?"\s*:\s*\\?")s-\d{4}-\d{2}-\d{2}-[0-9a-fA-F]+(\\?")')

MCP_INIT_PARAMS = {
    "protocolVersion": "2024-11-05",
    "capabilities": {},
    "clientInfo": {"name": "governance-compat", "version": "0.0"},
}


def fixture_text(name: str) -> str:
    return (FIXTURES / name).read_text()


def sha256_file(path: Path) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(65536), b""):
            h.update(chunk)
    return h.hexdigest()


def git_rev(src_dir: str) -> str:
    try:
        out = subprocess.run(["git", "-C", src_dir, "rev-parse", "HEAD"],
                             capture_output=True, text=True, timeout=30)
        return out.stdout.strip() if out.returncode == 0 else f"unavailable ({out.stderr.strip()})"
    except Exception as exc:  # pragma: no cover - defensive
        return f"unavailable ({exc})"


def normalize(text: str, root: Path) -> str:
    root_s = str(root)
    text = text.replace(root_s + "/", "<ROOT>/").replace(root_s, "<ROOT>")
    text = NAMED_TS.sub(r"\1<TS>", text)
    text = SESSION_ID.sub(r"\1<SESSION>\2", text)
    return text


def setup_project(case: dict, variant: dict | None = None) -> Path:
    if variant is None:
        variant = {}
    if variant:
        case = {**case, **variant}
    else:
        case = dict(case)
    root = Path(tempfile.mkdtemp(prefix="compat-proj-"))
    phr = root / ".phronesis"
    phr.mkdir()
    rules = case.get("rules")
    if rules:
        (phr / "rules.json").write_text(fixture_text(rules))
    for rel, content in (case.get("extra") or {}).items():
        target = root / rel
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(content)
    for rel, fixture in (case.get("providers") or {}).items():
        target = phr / "predicates" / rel
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(fixture_text(fixture))
    loader = case.get("loader")
    if loader:
        layers = []
        if "inline" in loader:
            (phr / "loader.json").write_text(json.dumps(loader["inline"]))
        else:
            for key, fname in (("base", loader["base"]), ("override", loader["override"])):
                layer_path = root / f"{key}-rules-layer.json"
                layer_path.write_text(fixture_text(fname))
                layers.append({"name": key, "path": str(layer_path)})
            for extra in loader.get("extra_layers") or []:
                layers.append(dict(extra))
            if loader.get("optional_override"):
                layers[1]["optional"] = True
            (phr / "loader.json").write_text(
                json.dumps({"version": 1, "layers": layers}, indent=2))
            if loader.get("delete_override"):
                Path(layers[1]["path"]).unlink(missing_ok=True)
    return root


def base_env(case: dict, root: Path) -> dict:
    env = {**os.environ}
    env["PHRONESIS_PROJECT_ROOT"] = str(root)
    if case.get("no_action_log", True):
        env["PHRONESIS_NO_ACTION_LOG"] = "1"
    for k, v in (case.get("env") or {}).items():
        env[k] = v
    return env


def apply_env(env: dict, overrides: dict | None) -> dict:
    env = dict(env)
    for k, v in (overrides or {}).items():
        if v == "":
            env.pop(k, None)
        else:
            env[k] = v
    return env


def run_call(bin_path: str, root: Path, call: dict, cmd_env: dict) -> dict:
    argv = [bin_path] + [str(a).replace("<ROOT>", str(root)) for a in call["argv"]]
    env = apply_env(cmd_env, call.get("env"))
    stdin_data = b""
    if "payload" in call:
        stdin_data = fixture_text(call["payload"]).encode()
    proc = subprocess.run(argv, input=stdin_data, env=env,
                          cwd=str(root), capture_output=True, timeout=120)
    return {
        "exit": proc.returncode,
        "stdout": proc.stdout.decode("utf-8", errors="replace"),
        "stderr": proc.stderr.decode("utf-8", errors="replace"),
    }


def read_disk(root: Path, relpaths: list) -> dict:
    out = {}
    for rel in relpaths:
        target = root / rel
        out[f"disk:{rel}"] = target.read_text(errors="replace") if target.exists() else "<absent>"
    return out


def label_call(index: int, call: dict) -> str:
    if "argv" in call:
        parts = [p for p in call["argv"] if isinstance(p, str)]
        return f"call-{index:02d}-" + re.sub(r"[^A-Za-z0-9._-]+", "-", "-".join(parts))[:60]
    return f"call-{index:02d}"


def check_expect(expect: dict, exit_code: int, stdout: str, stderr: str) -> list:
    problems = []
    if "exit" in expect and exit_code != expect["exit"]:
        problems.append(f"exit {exit_code} != {expect['exit']}; stderr: {stderr[:200]!r}")
    if "exit_not" in expect and expect["exit_not"] is not None and exit_code == expect["exit_not"]:
        problems.append(f"exit {exit_code} == forbidden {expect['exit_not']}")
    for needle in expect.get("stdout") or []:
        if needle not in stdout:
            problems.append(f"stdout missing {needle!r}")
    for needle in expect.get("stdout_not") or []:
        if needle == "any":
            if stdout.strip():
                problems.append(f"stdout not empty: {stdout[:120]!r}")
        elif needle in stdout:
            problems.append(f"stdout unexpectedly contains {needle!r}")
    for needle in expect.get("stderr") or []:
        if needle not in stderr:
            problems.append(f"stderr missing {needle!r}; got {stderr[:200]!r}")
    for needle in expect.get("stderr_not") or []:
        if needle == "any":
            if stderr.strip():
                problems.append(f"stderr not empty: {stderr[:120]!r}")
        elif needle in stderr:
            problems.append(f"stderr unexpectedly contains {needle!r}")
    return problems


def check_disk_expect(spec: dict, root: Path) -> list:
    target = root / spec["path"]
    problems = []
    if spec.get("absent"):
        if target.exists():
            problems.append(f"{spec['path']} should be absent but exists")
        return problems
    if not target.exists():
        if spec.get("not_contains"):
            return problems
        problems.append(f"{spec['path']} should exist but is absent")
        return problems
    content = target.read_text(errors="replace")
    for needle in spec.get("contains") or []:
        if needle not in content:
            problems.append(f"{spec['path']} missing {needle!r}")
    for needle in spec.get("not_contains") or []:
        if needle in content:
            problems.append(f"{spec['path']} unexpectedly contains {needle!r}")
    return problems


def run_case_side(bin_path: str, case: dict, side_dir: Path) -> tuple[dict, dict, list, list]:
    """Run one case against one binary.

    Returns (raw_artifacts, normalized_artifacts, problems, assertion_failures).
    raw_artifacts hold verbatim outputs; normalized_artifacts hold the same
    keys normalized with the per-run temp root.
    """
    raw: dict[str, str] = {}
    norm: dict[str, str] = {}
    problems: list[str] = []
    assertions: list[str] = []
    side_dir.mkdir(parents=True, exist_ok=True)
    variants = case.get("variants") or [None]
    for variant in variants:
        root = setup_project(case, variant)
        try:
            suffix = f"--{variant['suffix']}" if variant and variant.get("suffix") else ""
            cmd_env = base_env(case, root)

            if "mcp" in case:
                mcp_raw, mcp_norm, mcp_probs, mcp_asserts = run_mcp_case(
                    bin_path, case, root, cmd_env, suffix)
                raw.update(mcp_raw)
                norm.update(mcp_norm)
                problems += mcp_probs
                assertions += mcp_asserts
            else:
                calls = (variant.get("calls") if variant and variant.get("calls")
                         else case.get("calls") or [])
                for i, call in enumerate(calls):
                    label = label_call(i, call) + suffix
                    result = run_call(bin_path, root, call, cmd_env)
                    raw[f"{label}.exit"] = str(result["exit"])
                    raw[f"{label}.out"] = result["stdout"]
                    raw[f"{label}.err"] = result["stderr"]
                    norm[f"{label}.out"] = normalize(result["stdout"], root)
                    norm[f"{label}.err"] = normalize(result["stderr"], root)
                    for problem in check_expect(call.get("expect") or {}, result["exit"],
                                                result["stdout"], result["stderr"]):
                        assertions.append(f"{case['name']}/{label}: {problem}")
                    for spec in call.get("expect_disk") or []:
                        for problem in check_disk_expect(spec, root):
                            assertions.append(f"{case['name']}/{label}: {problem}")

            for i, call in enumerate(case.get("calls_after") or []):
                label = label_call(90 + i, call) + suffix
                result = run_call(bin_path, root, call, cmd_env)
                raw[f"{label}.exit"] = str(result["exit"])
                raw[f"{label}.out"] = result["stdout"]
                raw[f"{label}.err"] = result["stderr"]
                norm[f"{label}.out"] = normalize(result["stdout"], root)
                norm[f"{label}.err"] = normalize(result["stderr"], root)
                for problem in check_expect(call.get("expect") or {}, result["exit"],
                                            result["stdout"], result["stderr"]):
                    assertions.append(f"{case['name']}/{label}: {problem}")

            for spec in case.get("expect_disk") or []:
                for problem in check_disk_expect(spec, root):
                    assertions.append(f"{case['name']}{suffix}: {problem}")
            for key, content in read_disk(root, case.get("disk") or []).items():
                raw[key + suffix] = content
                norm[key + suffix] = normalize(content, root)
        finally:
            shutil.rmtree(root, ignore_errors=True)
    for key, value in raw.items():
        (side_dir / re.sub(r"[^A-Za-z0-9._-]+", "__", key)).write_text(value)
    return raw, norm, problems, assertions


class McpConn:
    def __init__(self, bin_path: str, root: Path, env: dict):
        self.proc = subprocess.Popen(
            [bin_path, "serve"], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=subprocess.PIPE, env=env, cwd=str(root), text=True)
        self.next_id = 0

    def _readline(self, timeout: float = 60.0) -> str:
        sel = selectors.DefaultSelector()
        sel.register(self.proc.stdout, selectors.EVENT_READ)
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            events = sel.select(timeout=max(0.1, deadline - time.monotonic()))
            if events:
                line = self.proc.stdout.readline()
                if line == "":
                    raise RuntimeError("server closed stdout")
                if line.endswith("\n"):
                    return line
        raise RuntimeError("timed out waiting for server response")

    def call(self, method: str, params: dict) -> dict:
        self.next_id += 1
        msg_id = self.next_id
        msg = json.dumps({"jsonrpc": "2.0", "id": msg_id, "method": method, "params": params})
        self.proc.stdin.write(msg + "\n")
        self.proc.stdin.flush()
        while True:
            value = json.loads(self._readline())
            if value.get("id") == msg_id:
                return value

    def notify(self, method: str, params: dict) -> None:
        msg = json.dumps({"jsonrpc": "2.0", "method": method, "params": params})
        self.proc.stdin.write(msg + "\n")
        self.proc.stdin.flush()

    def close(self) -> None:
        try:
            self.proc.stdin.close()
        except Exception:
            pass
        self.proc.terminate()
        try:
            self.proc.wait(timeout=10)
        except Exception:
            self.proc.kill()


def run_mcp_case(bin_path: str, case: dict, root: Path, cmd_env: dict,
                 suffix: str) -> tuple[dict, dict, list, list]:
    raw: dict[str, str] = {}
    norm: dict[str, str] = {}
    problems: list[str] = []
    assertions: list[str] = []
    name = case["name"]
    conn = McpConn(bin_path, root, cmd_env)
    try:
        resp = conn.call("initialize", MCP_INIT_PARAMS)
        text = json.dumps(resp, sort_keys=True, indent=1)
        raw[f"mcp-00-initialize{suffix}.out"] = text
        norm[f"mcp-00-initialize{suffix}.out"] = normalize(text, root)
        conn.notify("notifications/initialized", {})
        for i, step in enumerate(case.get("mcp") or []):
            label = f"mcp-{i + 1:02d}-"
            expect = step.get("expect") or {}
            if "write_disk" in step:
                target = root / step["write_disk"]["path"]
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_text(fixture_text(step["write_disk"]["fixture"]))
                label += "write-disk"
                raw[label + suffix + ".out"] = "ok"
                norm[label + suffix + ".out"] = "ok"
            elif "tool" in step:
                label += step["tool"]
                resp = conn.call("tools/call", {"name": step["tool"], "arguments": step["args"]})
                text = json.dumps(resp, sort_keys=True, indent=1)
                inner = ""
                for item in (resp.get("result", {}) or {}).get("content") or []:
                    if isinstance(item, dict) and isinstance(item.get("text"), str):
                        inner += item["text"] + "\n"
                haystack = text + "\n" + inner
                raw[label + suffix + ".out"] = text
                norm[label + suffix + ".out"] = normalize(text, root)
                is_error = "error" in resp
                is_tool_error = resp.get("result", {}).get("isError") is True
                if expect.get("ok") and (is_error or is_tool_error):
                    assertions.append(
                        f"{name}/{label}: expected tool success, got error: {text[:300]!r}")
                if expect.get("error") and not is_error:
                    assertions.append(
                        f"{name}/{label}: expected JSONRPC error, got success: {text[:300]!r}")
                if expect.get("error") and is_tool_error:
                    assertions.append(
                        f"{name}/{label}: expected schema error (error key), "
                        f"got isError result: {text[:300]!r}")
                for needle in expect.get("contains") or []:
                    if needle not in haystack:
                        assertions.append(f"{name}/{label}: response missing {needle!r}")
                for needle in expect.get("not_contains") or []:
                    if needle in haystack:
                        assertions.append(
                            f"{name}/{label}: response unexpectedly contains {needle!r}")
            else:
                label += step["method"].replace("/", "-")
                resp = conn.call(step["method"], step["params"])
                text = json.dumps(resp, sort_keys=True, indent=1)
                raw[label + suffix + ".out"] = text
                norm[label + suffix + ".out"] = normalize(text, root)
                for needle in expect.get("contains") or []:
                    if needle not in text:
                        assertions.append(f"{name}/{label}: response missing {needle!r}")
                for needle in expect.get("not_contains") or []:
                    if needle in text:
                        assertions.append(
                            f"{name}/{label}: response unexpectedly contains {needle!r}")
            if "expect_disk" in step:
                for problem in check_disk_expect(step["expect_disk"], root):
                    assertions.append(f"{name}/{label}: {problem}")
    finally:
        conn.close()
    return raw, norm, problems, assertions


def unified_diff(name: str, a: str, b: str) -> str:
    return "".join(difflib.unified_diff(
        a.splitlines(keepends=True), b.splitlines(keepends=True),
        fromfile=f"baseline/{name}", tofile=f"candidate/{name}"))


def main() -> int:
    parser = argparse.ArgumentParser(description="phr-mcp governance differential runner")
    parser.add_argument("--baseline-bin", required=True)
    parser.add_argument("--candidate-bin", required=True)
    parser.add_argument("--output-dir", required=True)
    parser.add_argument("--baseline-src", default=None,
                        help="baseline source checkout for git rev")
    parser.add_argument("--candidate-src", default=None,
                        help="candidate source checkout for git rev")
    args = parser.parse_args()

    baseline_bin = Path(args.baseline_bin).resolve()
    candidate_bin = Path(args.candidate_bin).resolve()
    out_dir = Path(args.output_dir).resolve()
    if not baseline_bin.exists():
        print(f"error: baseline binary not found: {baseline_bin}", file=sys.stderr)
        return 2
    if not candidate_bin.exists():
        print(f"error: candidate binary not found: {candidate_bin}", file=sys.stderr)
        return 2
    for side in ("baseline", "candidate", "normalized", "diffs"):
        (out_dir / side).mkdir(parents=True, exist_ok=True)

    report = {
        "generated_by": "scripts/governance-compat/diff_runner.py",
        "baseline_bin": str(baseline_bin),
        "candidate_bin": str(candidate_bin),
        "baseline_bin_sha256": sha256_file(baseline_bin),
        "candidate_bin_sha256": sha256_file(candidate_bin),
        "baseline_src_rev": git_rev(args.baseline_src) if args.baseline_src else None,
        "candidate_src_rev": git_rev(args.candidate_src) if args.candidate_src else None,
        "fixtures_sha256": {p.name: sha256_file(p)
                            for p in sorted(FIXTURES.rglob("*")) if p.is_file()},
        "normalization": ["fixture temp project root -> <ROOT>",
                          "named timestamp fields (ts, ts_ms, created_at_ms, "
                          "updated_at_ms, expires_at, emitted_at) -> <TS>",
                          "generated session_id s-YYYY-MM-DD-<hex> -> <SESSION>"],
        "raw_outputs": "verbatim under baseline/ and candidate/ (unnormalized)",
        "cases": {},
    }

    mismatches: list[str] = []
    operational: list[str] = []
    assertion_failures: list[str] = []
    ran = set()

    for case in CASES:
        name = case["name"]
        report["cases"][name] = {}
        try:
            base_raw, base_norm, base_probs, base_asserts = run_case_side(
                str(baseline_bin), case, out_dir / "baseline" / name)
            cand_raw, cand_norm, cand_probs, cand_asserts = run_case_side(
                str(candidate_bin), case, out_dir / "candidate" / name)
        except Exception as exc:
            operational.append(f"{name}: {type(exc).__name__}: {exc}")
            report["cases"][name]["error"] = str(exc)
            continue
        ran.add(name)
        for problem in base_probs + cand_probs:
            operational.append(f"{name}: {problem}")
        for failure in base_asserts:
            assertion_failures.append(f"baseline: {failure}")
        for failure in cand_asserts:
            assertion_failures.append(f"candidate: {failure}")
        for key in sorted(set(base_norm) | set(cand_norm)):
            a = base_norm.get(key, "<missing>")
            b = cand_norm.get(key, "<missing>")
            safe = re.sub(r"[^A-Za-z0-9._-]+", "__", key)
            n_base = out_dir / "normalized" / "baseline" / name
            n_cand = out_dir / "normalized" / "candidate" / name
            n_base.mkdir(parents=True, exist_ok=True)
            n_cand.mkdir(parents=True, exist_ok=True)
            (n_base / safe).write_text(a)
            (n_cand / safe).write_text(b)
            if a == b:
                report["cases"][name][key] = "match"
            else:
                report["cases"][name][key] = "MISMATCH"
                mismatches.append(f"{name}:{key}")
                diff_dir = out_dir / "diffs" / name
                diff_dir.mkdir(parents=True, exist_ok=True)
                (diff_dir / (safe + ".diff")).write_text(unified_diff(key, a, b))

    missing_required = [c for c in REQUIRED_CASES if c not in ran]
    report["summary"] = {
        "cases_run": len(ran),
        "required_cases": len(REQUIRED_CASES),
        "missing_required": missing_required,
        "mismatch_count": len(mismatches),
        "mismatches": mismatches,
        "assertion_failure_count": len(assertion_failures),
        "assertion_failures": assertion_failures,
        "operational_errors": operational,
    }
    (out_dir / "report.json").write_text(json.dumps(report, indent=1, sort_keys=True))

    lines = [f"cases run: {len(ran)}/{len(REQUIRED_CASES)} required"]
    if mismatches:
        lines.append(f"MISMATCHES ({len(mismatches)}):")
        lines += [f"  - {m}" for m in mismatches]
    else:
        lines.append("all compared artifacts match")
    if assertion_failures:
        lines.append(f"ASSERTION FAILURES ({len(assertion_failures)}):")
        lines += [f"  - {a}" for a in assertion_failures]
    if missing_required:
        lines.append(f"MISSING REQUIRED CASES: {missing_required}")
    if operational:
        lines.append(f"OPERATIONAL ERRORS ({len(operational)}):")
        lines += [f"  - {o}" for o in operational]
    text = "\n".join(lines) + "\n"
    (out_dir / "report.txt").write_text(text)
    print(text, end="")

    if missing_required:
        return 3
    if assertion_failures:
        return 3
    if operational:
        return 2
    if mismatches:
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())