use serde_json::{Value, json};

/// Regex fragment matching a `git` invocation in a shell command line, up to
/// (but not including) its subcommand. Every packaged rule that gates a git
/// subcommand builds its `bash_command_matches` pattern from this fragment
/// rather than hand-rolling `git\s+<subcommand>` — that literal shape only
/// recognizes `git` immediately followed by the subcommand, so
/// `git -C . commit`, `git -c user.name=x commit`,
/// `git --git-dir=.git commit`, `git --no-pager commit`,
/// `/usr/bin/git commit`, `\git commit`, `command git commit`, and
/// `env FOO=bar git commit` all bypassed every gate silently (C18).
///
/// Recognizes, in any order and any number of times, git's common global
/// options (`-C <dir>`, `-c <k=v>`, `--git-dir=…`, `--work-tree=…`,
/// `--namespace=…`, `--exec-path[=…]`, `--no-pager`, `--paginate`/`-p`,
/// `--bare`, `-P`, `--literal-pathspecs`, `--no-optional-locks`,
/// `--no-lazy-fetch`) between the binary and the subcommand, plus a
/// `command`/`env FOO=bar` wrapper, a backslash-escaped binary name, and an
/// absolute or relative path to the binary.
///
/// The leading `(?:^|[;&|]\s*)` anchor requires the invocation to start the
/// command line or immediately follow a shell separator (`;`, `&&`, `||`,
/// `|`) — never trail an unrelated word or an open quote — which is what
/// keeps this from matching `git` mentioned inside `echo "git commit"`.
pub(super) fn git_invocation_prefix() -> &'static str {
    r"(?:^|[;&|]\s*)(?:(?:command|exec)\s+)?(?:env\s+(?:\S+=\S+\s+)*)?\\?(?:\S*/)?git\b(?:\s+(?:-c\s+\S+|-C\s+\S+|--git-dir=\S+|--work-tree=\S+|--namespace=\S+|--exec-path(?:=\S+)?|--no-pager|--paginate|--bare|--literal-pathspecs|--no-optional-locks|--no-lazy-fetch|-p|-P))*"
}

/// Build a `bash_command_matches` pattern recognizing any of `subcommands`
/// (a `|`-joined regex alternation, e.g. `"commit|merge|rebase"`) as git's
/// *subcommand*, tolerant of everything `git_invocation_prefix` handles.
///
/// Plumbing extensions of a name (`commit-tree`, `merge-base`) are
/// deliberately excluded by requiring the subcommand to be followed by
/// whitespace or end-of-string rather than any word/hyphen character: they
/// don't mutate history/branches the way the porcelain command does
/// (`commit-tree` writes a commit object without touching any ref or the
/// index), so gating them would be a false positive, not a closed bypass.
/// (The `regex` crate has no look-around, hence `(?:\s|$)` rather than a
/// negative lookahead.)
pub(super) fn git_subcommand_gate(subcommands: &str) -> String {
    format!(r"{}\s+(?:{subcommands})(?:\s|$)", git_invocation_prefix())
}

/// Confidence-scoring gate rules (SPEC-confidence-scoring §3, approach A;
/// severity per SPEC-structural-rule-migration §"Confidence gate severity").
/// They count the open work unit's passed `signal_pass` facts (asserted by the
/// pre-check hook) and advise on a governed Git mutation by band: ≤1 signal
/// warns (missing/failing evidence), exactly 2 warns (one grounded signal
/// missing), 3 passes clean. Neither band blocks — incomplete or failing
/// confidence evidence is observability, not enforcement, so the Git command
/// always proceeds. Paired with the `.phronesis/confidence.json` marker +
/// `.phronesis/bugs.json` registry written by `write_confidence_scaffold`.
///
/// The aggregate `signal_pass` count cannot distinguish "never ran" from "ran
/// and failed" (both are absent facts), so neither message names a specific
/// missing or failing signal — that would claim more than the asserted facts
/// prove. Run `phr-mcp confidence` for the itemized per-signal report.
pub(super) fn confidence_rules() -> Value {
    let commit_gate = git_subcommand_gate("commit|merge|rebase|cherry-pick|revert|pull");
    json!({
        "rules": [
            {
                "id": "confidence-low-blocks-commit",
                "phase": "pre",
                "priority": 30,
                "when": [
                    {"bash_command_matches": commit_gate.clone()},
                    {"__script__": "facts_count('signal_pass', ['*','*']) <= 1"}
                ],
                "then": {"warn": "Low confidence — compile/tests/known-bug evidence is incomplete or failing. Run `phr-mcp confidence` for the per-signal report before presenting this as done."}
            },
            {
                "id": "confidence-medium-warns-commit",
                "phase": "pre",
                "priority": 29,
                "when": [
                    {"bash_command_matches": commit_gate},
                    {"__script__": "facts_count('signal_pass', ['*','*']) == 2"}
                ],
                "then": {"warn": "Medium confidence — one grounded signal is missing. Review before presenting this as done."}
            }
        ]
    })
}

/// The `bash_command_code_matches` regex for shell writes to the
/// verification trust anchors (SPEC-verification-artifact-generation S1).
/// It runs on `hook_facts::shell_code_text`, so heredoc bodies (commit
/// messages, documents being written) are already gone and `sh -c` scripts
/// are unwrapped. Lexical and therefore advisory — the rule WARNS; the
/// file-tool rules are the enforced seam — but tuned so everyday commands
/// stay silent:
///
/// - Anchor tokens are root-level: `.phronesis/verification(-allowlist).json`
///   and `verification/templates`, bare, `./`, or `$PWD/`, matched
///   case-insensitively (the default macOS filesystem folds case), each
///   starting and ending at a token boundary. `/tmp/.phronesis/…`,
///   `fixtures/email-verification.json`, `src/verification/templates.rs`,
///   and `verification.json.bak` are not anchors.
/// - Every alternative must start outside quotes: a leading prefix consumes
///   whole quoted strings, so anchor text inside `echo "…"` or a
///   `git commit -m "…"` message never matches.
/// - `cp`/`mv`/`install`/`ln`/`rsync` match only when the anchor is the
///   DESTINATION (the last argument, `-t`, or a `.phronesis/` / `verification/`
///   directory receiving a same-named source) — copying an anchor out is a read.
/// - Redirects, `tee`/`unlink`/`truncate`/`touch`/`patch`/`shred`, `rm`
///   (not `--cached`), `git restore` (not `--staged`), `git checkout -- …`,
///   in-place `sed`/`perl`/`ruby`, `dd of=`, and `curl -o`/`wget -O` match an
///   anchor argument; a `<` input redirect is a read and ends the scan.
///   `cd .phronesis` then a write to a bare `verification(-allowlist).json`
///   matches until the next `cd`.
/// - Commands may be prefixed with `VAR=value`, `sudo`, `env`, `command`,
///   `exec`, `nohup`, `time`, `xargs`, or `git`.
pub(super) fn trust_anchor_shell_pattern() -> String {
    // Outside-quotes prefix: whole quoted strings or single unquoted chars.
    const Q: &str = r#"(?s)\A(?:[^"'\\]|\\.|"(?:[^"\\]|\\.)*"|'[^']*')*?"#;
    // Command position: segment start, then env assignments and wrappers.
    const CMD: &str = r#"(?:\A|[;&|({`\n])\s*(?:(?:[A-Za-z_][A-Za-z0-9_]*=\S*|sudo|env|command|exec|xargs|nohup|time|git|then|do|else)\s+(?:-\S*\s+)*)*"#;
    // The project root, spelled bare, `./`, or `$PWD/`.
    const ROOT: &str = r#"(?:\./|\$\{?PWD\}?/|\$\(pwd\)/)?"#;
    const JSON_NAME: &str = r#"(?i:verification(?:-allowlist)?\.json)"#;
    // Token end, and destination end (last argument of the segment).
    const END: &str = r#"["']?(?:[\s;&|<>)`]|\z)"#;
    const DEST_END: &str = r#"["']?\s*(?:\d?>[^;&|\n]*)?(?:[;&|)`\n]|\z)"#;
    // Any preceding arguments (never across a `<` input redirect), then the
    // start of the anchor argument.
    const ARGS: &str = r#"\s(?:[^;&|<\n]*?\s)?["']?"#;
    const REST: &str = r#"[^\s;&|<]*"#;

    let json_anchor = format!(r#"{ROOT}(?i:\.phronesis)/{JSON_NAME}"#);
    let templates = format!(r#"{ROOT}(?i:verification/templates)(?:/[^\s;&|<>"'()]*)?"#);
    let anchor = format!("(?:{json_anchor}|{templates})");
    // An argument token other than `--cached` / `--staged` (index-only).
    let not_cached = format!(
        r#"(?:[^\s;&|<\-]{REST}|-[^\s;&|<\-]{REST}|--(?:[^\s;&|<c]{REST}|c[^\s;&|<a]{REST})?)"#
    );
    let not_staged = format!(
        r#"(?:[^\s;&|<\-]{REST}|-[^\s;&|<\-S]{REST}|--(?:[^\s;&|<s]{REST}|s[^\s;&|<t]{REST})?)"#
    );
    let token = r#"[^\s;&|<]+"#;
    let alternatives = [
        // Redirects: > >> >| &> 2>
        format!(r#"(?:\d|&)?>>?\|?\s*["']?{anchor}{END}"#),
        // Writers of every named argument.
        format!(r#"{CMD}(?:tee|unlink|truncate|touch|patch|shred){ARGS}{anchor}{END}"#),
        // rm, but not the index-only `git rm --cached`.
        format!(r#"{CMD}rm(?:\s+{not_cached})*?\s+["']?{anchor}{END}"#),
        // git restore, but not the index-only `--staged`.
        format!(r#"{CMD}restore(?:\s+{not_staged})*?\s+["']?{anchor}{END}"#),
        // git checkout of paths (after `--`), never a branch name.
        format!(r#"{CMD}checkout(?:\s+{token})*?\s+--(?:\s+{token})*?\s+["']?{anchor}{END}"#),
        // Copiers: the anchor is the last argument.
        format!(r#"{CMD}(?:cp|mv|install|ln|rsync){ARGS}{anchor}{DEST_END}"#),
        // Copiers: `-t` / `--target-directory` names the templates anchor.
        format!(
            r#"{CMD}(?:cp|mv|install|ln)(?:\s[^;&|<\n]*?)?\s(?:-t\s*|--target-directory[=\s])["']?{templates}{END}"#
        ),
        // Copiers: a same-named source into the `.phronesis/` directory.
        format!(
            r#"{CMD}(?:cp|mv|install|ln|rsync)\s[^;&|<\n]*?[\s/"']{JSON_NAME}["']?{ARGS}{ROOT}(?i:\.phronesis)/?{DEST_END}"#
        ),
        // Copiers: a `templates` source into the root `verification/` directory.
        format!(
            r#"{CMD}(?:cp|mv|install|ln|rsync)\s[^;&|<\n]*?[\s/"'](?i:templates)/?["']?{ARGS}{ROOT}(?i:verification)/?{DEST_END}"#
        ),
        // In-place editors.
        format!(
            r#"{CMD}(?:sed|perl|ruby)\s(?:[^;&|<\n]*?\s)?(?:-[A-Za-z]*i\S*|--in-place\S*){ARGS}{anchor}{END}"#
        ),
        // dd of=
        format!(r#"{CMD}dd\s[^;&|<\n]*?\bof=["']?{anchor}{END}"#),
        // curl -o / wget -O
        format!(
            r#"{CMD}(?:curl|wget)(?:\s[^;&|<\n]*?)?\s(?:-[A-Za-z]*[oO]\s*|--output(?:-document)?[=\s])["']?{anchor}{END}"#
        ),
        // `cd .phronesis`, then — before any other `cd` — a write to a bare
        // anchor file name.
        format!(
            r#"(?:\A|[;&|({{`\n])\s*cd\s+["']?{ROOT}(?i:\.phronesis)/?["']?\s*(?:&&|;|\n)(?:[^c]|c+[^cd]|c+d\S)*?(?:>>?\|?\s*|(?:tee|rm|touch|truncate|sed\s+-i\S*)\s(?:[^;&|<\n]*?\s)?)["']?(?:\./)?{JSON_NAME}{END}"#
        ),
    ];
    format!("{Q}(?:{})", alternatives.join("|"))
}

pub(super) fn deflection_rules() -> Value {
    let commit_gate = git_subcommand_gate("commit");
    let add_all_gate = format!(r"{}\s+add\s+(?:-A\b|\.(?:$|\s))", git_invocation_prefix());
    let shell_anchor_pattern = trust_anchor_shell_pattern();
    json!({
        "rules": [
            {
                "id": "enforce-no-pre-existing-issue",
                "phase": "pre",
                "priority": 10,
                "when": [
                    {"new_content_contains": "pre-existing issue"}
                ],
                "then": {"block": "Don't deflect with 'pre-existing issue'. Either fix it as part of this change, defer with a clear rationale, or drop the disclaimer."}
            },
            {
                "id": "enforce-no-not-from-our-changes",
                "phase": "pre",
                "priority": 10,
                "when": [
                    {"new_content_contains": "not from our changes"}
                ],
                "then": {"block": "Drop the 'not from our changes' disclaimer. Name the issue and decide: fix or defer."}
            },
            {
                "id": "enforce-no-not-caused-by-our",
                "phase": "pre",
                "priority": 10,
                "when": [
                    {"new_content_contains": "not caused by our"}
                ],
                "then": {"block": "Drop the 'not caused by our' disclaimer. Own the fix or own the decision to defer."}
            },
            {
                "id": "enforce-no-should-work-claim",
                "phase": "pre",
                "priority": 10,
                "when": [
                    {"new_content_contains": "should work now"}
                ],
                "then": {"block": "Avoid claiming a fix is complete without evidence. Run the verification (test, manual exercise, traced call chain) before reporting, or explicitly label the work 'untested' so the human knows to check."}
            },
            {
                "id": "enforce-no-should-be-fixed-claim",
                "phase": "pre",
                "priority": 10,
                "when": [
                    {"new_content_contains": "should be fixed"}
                ],
                "then": {"block": "Don't make repair claims without verifying. Run the failing case end-to-end before reporting a fix; otherwise mark it 'untested' so the user knows to verify."}
            },
            {
                "id": "nudge-verify-before-commit",
                "phase": "pre",
                "priority": 5,
                "when": [
                    {"bash_command_matches": commit_gate},
                    {"__script__": "facts_count('confidence_enabled', []) == 0"}
                ],
                "then": {"warn": "About to commit. Trace the call chain end-to-end before reporting done. Half-fixes where one layer is wired but another is not are a recurring failure mode."}
            },
            {
                "id": "llm-warn-git-add-all",
                "phase": "pre",
                "priority": 5,
                "when": [
                    {"bash_command_matches": add_all_gate}
                ],
                "then": {"warn": "Stage files explicitly — git add -A / git add . sweeps unrelated changes into the commit. List the files you actually changed."}
            },
            {
                "id": "llm-warn-kill-build",
                "phase": "pre",
                "priority": 5,
                "when": [
                    {"bash_command_matches": "\\b(pkill|killall|kill)\\b[^;|&]*\\b(cargo|rustc)\\b"}
                ],
                "then": {"warn": "Builds are I/O-bound: a rustc at 0% CPU is usually in disk-wait, not hung. Give it time or check `ps` state before killing the build."}
            },
            // SPEC-verification-artifact-generation.md S1/S3 (acceptance C7):
            // the verification trust anchors are human-principal acts, so the
            // agent seam is refused. File tools are matched on the path
            // relative to the project root (`project_path_is` /
            // `project_path_under`), so only the root-level anchors match,
            // never a lookalike nested elsewhere; those rules block. Shell
            // writes are matched on the command's code by
            // `trust_anchor_shell_pattern`, which is lexical: a write through
            // an interpreter or a variable that never spells the anchor path
            // is not caught, so that seam only warns (the spec's enforcement
            // note).
            {
                "id": "block-agent-write-to-verification-allowlist",
                "phase": "pre",
                "priority": 100,
                "when": [
                    {"project_path_is": ".phronesis/verification-allowlist.json"}
                ],
                "then": {"block": "The verification allowlist is a trust anchor: approvals are human-principal acts (SPEC-C S3). An approval written by the agent is not a review — ask the human to approve the artifact."}
            },
            {
                "id": "block-agent-write-to-verification-optin",
                "phase": "pre",
                "priority": 100,
                "when": [
                    {"project_path_is": ".phronesis/verification.json"}
                ],
                "then": {"block": "`.phronesis/verification.json` is the verification opt-in (including `raw_execution`) and a trust anchor (SPEC-C S1). Only the human changes it."}
            },
            {
                "id": "block-agent-write-to-verification-templates",
                "phase": "pre",
                "priority": 100,
                "when": [
                    {"project_path_under": "verification/templates"}
                ],
                "then": {"block": "`verification/templates/` is a trust anchor (SPEC-C S1/S3): templates and the devcontainer declaration are human-principal content. Propose the change to the human instead of writing it."}
            },
            {
                "id": "warn-agent-shell-write-to-trust-anchors",
                "phase": "pre",
                "priority": 100,
                "when": [
                    {"bash_command_code_matches": shell_anchor_pattern}
                ],
                "then": {"warn": "This command appears to write a verification trust anchor (`.phronesis/verification-allowlist.json`, `.phronesis/verification.json`, or `verification/templates/`). Those are human-principal acts (SPEC-C S1/S3) — reading them is fine; changing them is the human's call. Stop and ask unless the human asked for this change."}
            }
        ]
    })
}
