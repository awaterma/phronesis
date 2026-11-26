//! `phr-mcp doctor` — read-only diagnosis of hook wiring and rules for a
//! project, with an optional live allow/block probe.
//!
//! Plain `doctor` (no `--probe`) never writes to disk and never shells out:
//! every check reads project files directly. `--probe` additionally spawns
//! the current executable against a *temporary, isolated* fixture (see
//! [`probe`]) to produce real allow/block evidence; that evidence is always
//! reported as probe evidence, never promoted to a claim about the real
//! project's hook wiring or host trust state.

pub mod probe;

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::security;

/// Verdict for one diagnostic check. `NotObserved` is the honest default
/// when the signal needed to decide is not available to a read-only,
/// local-file check — never guessed as `Ok` or `Fail`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Ok,
    Warn,
    Fail,
    NotObserved,
}

#[derive(Debug, Clone, Serialize)]
pub struct Finding {
    pub id: &'static str,
    pub status: Status,
    pub message: String,
}

impl Finding {
    fn new(id: &'static str, status: Status, message: impl Into<String>) -> Self {
        Self {
            id,
            status,
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct DoctorReport {
    pub host: String,
    pub path: String,
    pub findings: Vec<Finding>,
    pub probe: Option<probe::ProbeReport>,
}

impl DoctorReport {
    /// 0 = every finding `Ok`/`NotObserved` (and, if run, probe matched
    /// expectations). 1 = at least one `Warn`. 2 = at least one `Fail`, or a
    /// probe that did not behave as the fixture rule demands.
    pub fn exit_code(&self) -> i32 {
        if self.findings.iter().any(|f| f.status == Status::Fail)
            || self.probe.as_ref().is_some_and(|p| !p.ok)
        {
            2
        } else if self.findings.iter().any(|f| f.status == Status::Warn) {
            1
        } else {
            0
        }
    }
}

pub struct DoctorOptions {
    pub path: PathBuf,
    pub host: String,
    pub probe: bool,
}

pub fn run(opts: &DoctorOptions) -> DoctorReport {
    let root = opts.path.clone();
    let findings = vec![
        check_binary_on_path(),
        check_governed(&root),
        check_rules_load(&root),
        check_host_hooks(&root, &opts.host),
        check_trust_observation(&opts.host),
        check_log_evidence(&root),
    ];

    let probe = opts.probe.then(probe::run);

    DoctorReport {
        host: opts.host.clone(),
        path: root.display().to_string(),
        findings,
        probe,
    }
}

/// Find `name` as a direct child of any directory in `path_var` (a
/// PATH-shaped string using the platform list separator). Pure and
/// side-effect-free so tests can pass a synthetic PATH instead of mutating
/// the real process environment — required for parallel-safe tests.
pub fn find_on_path(name: &str, path_var: &str) -> Option<PathBuf> {
    std::env::split_paths(path_var)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

/// The bare command name every hook config writes (`writers_hooks.rs`),
/// independent of platform file extension — used both for the PATH lookup
/// (with `.exe` on Windows) and for recognizing an invocation in a hook
/// `command` string (never with `.exe` there: that string is user-typed
/// shell text, not a filesystem lookup).
const PHR_MCP_BIN: &str = "phr-mcp";
const BIN_NAME: &str = if cfg!(windows) {
    "phr-mcp.exe"
} else {
    PHR_MCP_BIN
};

/// This binary's own version — reported so a WARN naming a *different*
/// PATH binary lets the reader compare versions, not just paths.
fn running_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

fn check_binary_on_path() -> Finding {
    let path_var = std::env::var("PATH").unwrap_or_default();
    let current_exe = std::env::current_exe().ok();
    let version = running_version();
    match find_on_path(BIN_NAME, &path_var) {
        None => Finding::new(
            "binary_on_path",
            Status::Warn,
            format!(
                "`{BIN_NAME}` not found on PATH; hooks configured with the bare command name will fail to launch. Running doctor at all implies some `phr-mcp` executable exists (this one: {} v{version}). Run `cargo install --path crates/phronesis-mcp` or otherwise put `phr-mcp` on PATH.",
                current_exe
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "<unknown>".to_string())
            ),
        ),
        Some(found) => {
            if !is_executable(&found) {
                return Finding::new(
                    "binary_on_path",
                    Status::Fail,
                    format!(
                        "`{BIN_NAME}` found on PATH at {} but is not executable (check its Unix file mode / `chmod +x`).",
                        found.display()
                    ),
                );
            }
            let same_as_current = current_exe
                .as_deref()
                .map(|cur| paths_likely_equal(cur, &found))
                .unwrap_or(false);
            if same_as_current {
                Finding::new(
                    "binary_on_path",
                    Status::Ok,
                    format!(
                        "`{BIN_NAME}` on PATH resolves to this running executable ({}, v{version}).",
                        found.display()
                    ),
                )
            } else {
                Finding::new(
                    "binary_on_path",
                    Status::Warn,
                    format!(
                        "`{BIN_NAME}` on PATH ({}) is a different binary than the one running this doctor check ({}, v{version}); hooks will run whichever version PATH finds, not necessarily this build. Reinstall with `cargo install --path crates/phronesis-mcp` if PATH should point at this build.",
                        found.display(),
                        current_exe
                            .map(|p| p.display().to_string())
                            .unwrap_or_else(|| "<unknown>".to_string())
                    ),
                )
            }
        }
    }
}

/// Unix: any execute bit set. Non-Unix (no execute-bit concept in the same
/// sense): treated as executable since `is_file` already passed — doctor has
/// no reliable signal to the contrary there.
#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|m| m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(_path: &Path) -> bool {
    true
}

fn paths_likely_equal(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

fn check_governed(root: &Path) -> Finding {
    if security::is_governed(root) {
        Finding::new(
            "project_governed",
            Status::Ok,
            format!("{} is a governed phronesis project.", root.display()),
        )
    } else {
        Finding::new(
            "project_governed",
            Status::Fail,
            format!(
                "{} has no `.phronesis/rules.json` or `loader.json`; no rules are enforced. Run `phr-mcp init`.",
                root.display()
            ),
        )
    }
}

/// Loads through the same read-only composition path the hook actually
/// uses at edit time (`crate::rule_layers::resolve`, which internally reads
/// language packs plus `loader.json`/`rules.json`) rather than a second,
/// primary-file-only diagnostic path — a project with language packs or a
/// `loader.json` layer gets the real rule count, not just the primary
/// file's. Read-only: `resolve` only reads files.
fn check_rules_load(root: &Path) -> Finding {
    let rules_path = crate::rules_file::default_path(root);
    let loader_path = crate::rule_layers::config_path(root);
    match crate::rule_layers::resolve(root) {
        Ok(resolved) if resolved.rules.is_empty() => Finding::new(
            "rules_load",
            Status::NotObserved,
            format!(
                "no rules resolved from {} (and {} if present, plus any installed language packs); nothing to load. Add an applicable rule or select starter packs with `phr-mcp init --packs <languages>`.",
                rules_path.display(),
                loader_path.display()
            ),
        ),
        Ok(resolved) => Finding::new(
            "rules_load",
            Status::Ok,
            format!(
                "effective rule set (project rules, {} when present, and any installed language packs) loaded cleanly: {} rule(s).",
                loader_path.display(),
                resolved.rules.len()
            ),
        ),
        Err(e) => Finding::new(
            "rules_load",
            Status::Fail,
            format!(
                "effective rule set failed to load: {e}. Repair the named configuration, then run doctor again."
            ),
        ),
    }
}

/// Which lifecycle commands a host's hook config is expected to wire, by
/// file path and host id. Only `claude` is checked today; other hosts
/// report `NotObserved` rather than a guessed verdict.
fn check_host_hooks(root: &Path, host: &str) -> Finding {
    match host {
        "claude" => check_claude_settings(root),
        other => Finding::new(
            "host_hooks",
            Status::NotObserved,
            format!(
                "doctor does not yet know how to inspect host `{other}`'s hook config. Inspect the host's hook settings and run an allowed/blocked edit in a disposable project."
            ),
        ),
    }
}

/// Whether a hook `command` string is **exactly** the two-token invocation
/// `phr-mcp <subcommand>` (the bare command, then the subcommand, and
/// nothing else). Absolute paths and wrappers require manual review: a
/// matching basename does not establish that a configured binary exists
/// or is the version checked by `binary_on_path`. Deliberately exact rather than a
/// substring or "subcommand appears somewhere" match: any extra token
/// before, between, or after changes what actually runs —
/// `echo phr-mcp pre-check` prints text and never calls `phr-mcp`;
/// `false && phr-mcp pre-check` only runs it when `false` (never) exits 0;
/// `phr-mcp pre-check; rm -rf x` runs something else afterward. None of
/// those are "wired" in the sense a project author means by it, even
/// though each contains a true invocation substring. Only the shipped
/// shape (`phr-mcp init`'s own `writers_hooks.rs` writes exactly this) is
/// accepted; anything else that merely mentions the subcommand is reported
/// `mentioned_unverified`, never treated as confirmed.
fn command_invokes(cmd: &str, subcommand: &str) -> bool {
    let tokens: Vec<&str> = cmd.split_whitespace().collect();
    let [bin, sub] = tokens.as_slice() else {
        return false;
    };
    *sub == subcommand && *bin == PHR_MCP_BIN
}

/// Verdict for one lifecycle event's wiring: `verified` is a recognized
/// `phr-mcp <subcommand>` invocation (see [`command_invokes`]), from a
/// `"type": "command"` entry that is not marked `async`; `mentioned_unverified`
/// is a command that merely contains the subcommand text without matching
/// that shape (a custom wrapper, an echo, a shell chain, a typo), a
/// matching command whose entry type is not `"command"`, or a matching
/// command marked `"async": true` (its execution relative to the tool call
/// is not something doctor can verify from config alone) — none of these
/// must ever be reported `ok`, only flagged for manual review.
/// `matcher_covers_edits` additionally requires the same entry's matcher to
/// match both actual tool names as a valid regular expression. A malformed
/// regex or lookalike tool name cannot establish edit coverage.
struct HookWire {
    verified: bool,
    mentioned_unverified: bool,
    matcher_covers_edits: bool,
}

fn matcher_covers_edit_and_write(matcher: &str) -> bool {
    regex::Regex::new(matcher)
        .map(|pattern| pattern.is_match("Edit") && pattern.is_match("Write"))
        .unwrap_or(false)
}

fn hook_wire(value: &serde_json::Value, event: &str, subcommand: &str) -> HookWire {
    let mut wire = HookWire {
        verified: false,
        mentioned_unverified: false,
        matcher_covers_edits: false,
    };
    let entries = value
        .get("hooks")
        .and_then(|h| h.get(event))
        .and_then(|e| e.as_array());
    for entry in entries.into_iter().flatten() {
        let matcher_covers = entry
            .get("matcher")
            .and_then(|m| m.as_str())
            .is_some_and(matcher_covers_edit_and_write);
        let Some(hooks) = entry.get("hooks").and_then(|h| h.as_array()) else {
            continue;
        };
        for h in hooks {
            let Some(cmd) = h.get("command").and_then(|c| c.as_str()) else {
                continue;
            };
            let is_command_type = h
                .get("type")
                .and_then(|t| t.as_str())
                .is_some_and(|t| t == "command");
            let synchronous = h
                .get("async")
                .is_none_or(|a| a == &serde_json::Value::Bool(false));
            if command_invokes(cmd, subcommand) && is_command_type && synchronous {
                wire.verified = true;
                wire.matcher_covers_edits |= matcher_covers;
            } else if cmd.contains(subcommand) || command_invokes(cmd, subcommand) {
                // Either the shape matched but type/async disqualified it,
                // or it merely mentions the subcommand without the exact
                // shape — either way, not confirmed.
                wire.mentioned_unverified = true;
            }
        }
    }
    wire
}

fn check_claude_settings(root: &Path) -> Finding {
    let path = root.join(".claude").join("settings.local.json");
    let Ok(text) = std::fs::read_to_string(&path) else {
        if root.join(".claude/settings.json").is_file() {
            return Finding::new(
                "host_hooks",
                Status::NotObserved,
                "generated .claude/settings.local.json is absent, but shared .claude/settings.json exists. Doctor does not resolve merged host settings; inspect /hooks in Claude Code to verify that configuration.",
            );
        }
        return Finding::new(
            "host_hooks",
            Status::Fail,
            format!(
                "{} not found; run `phr-mcp init` (or `--hooks-only`).",
                path.display()
            ),
        );
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Finding::new(
            "host_hooks",
            Status::Fail,
            format!(
                "{} is not valid JSON; run `phr-mcp init --hooks-only` to rewrite it, after resolving any manual edits you want to keep.",
                path.display()
            ),
        );
    };
    if value
        .get("disableAllHooks")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        return Finding::new(
            "host_hooks",
            Status::Fail,
            format!(
                "{} sets `disableAllHooks: true`; hooks are configured but will not run. Remove that setting.",
                path.display()
            ),
        );
    }

    let pre = hook_wire(&value, "PreToolUse", "pre-check");
    let post = hook_wire(&value, "PostToolUse", "post-check");

    if pre.verified && post.verified {
        if pre.matcher_covers_edits && post.matcher_covers_edits {
            Finding::new(
                "host_hooks",
                Status::Ok,
                format!(
                    "{} wires both `pre-check` and `post-check` with a matcher covering Edit/Write. Other settings scopes and the effective host configuration are not inspected; verify /hooks in the host.",
                    path.display()
                ),
            )
        } else {
            Finding::new(
                "host_hooks",
                Status::Warn,
                format!(
                    "{}: `pre-check`/`post-check` are wired, but their matcher does not clearly cover both `Edit` and `Write`; file edits may bypass them. Run `phr-mcp init --hooks-only` to restore the default matcher.",
                    path.display()
                ),
            )
        }
    } else if pre.mentioned_unverified || post.mentioned_unverified {
        Finding::new(
            "host_hooks",
            Status::Warn,
            format!(
                "{}: found a command mentioning `pre-check`/`post-check` that is not a recognized `phr-mcp pre-check`/`phr-mcp post-check` invocation (custom wrapper, alias, or typo) — unverified, not confirmed wired. Review manually.",
                path.display()
            ),
        )
    } else {
        let missing = match (pre.verified, post.verified) {
            (false, false) => "pre-check and post-check are",
            (false, true) => "pre-check is",
            (true, false) => "post-check is",
            (true, true) => unreachable!(),
        };
        Finding::new(
            "host_hooks",
            Status::Fail,
            format!(
                "{}: {missing} not wired. Run `phr-mcp init --hooks-only`.",
                path.display()
            ),
        )
    }
}

/// Whether the host actually trusts/loads this project's hooks (e.g. Claude
/// Code folder trust, Gemini folder trust, Codex `/hooks` approval) is not
/// readable from a local config file doctor controls — a config entry can
/// exist and still be un-trusted by the host UI. A default of "not
/// observed" is deliberately chosen over guessing `Ok`.
fn check_trust_observation(host: &str) -> Finding {
    Finding::new(
        "host_trust",
        Status::NotObserved,
        format!(
            "whether {host} currently trusts/loads this project's hooks is not observable from local files; doctor reports configuration only, not live host trust state. Review the host's trust and hook settings, then observe an allowed and a blocked edit in a disposable project."
        ),
    )
}

/// `.phronesis/log.jsonl` is unauthenticated local evidence of past hook
/// decisions — useful context, never proof that the *current* config is
/// correct or that the host is currently trusting these hooks. Reads a
/// small bounded tail via [`crate::action_log::read_recent`] (the same
/// reader `stats`/`get_action_log` use) rather than the whole file: a
/// long-lived project's log is not a size doctor should pay to read in
/// full just to report "was there a recent hook event".
fn check_log_evidence(root: &Path) -> Finding {
    let path = crate::action_log::default_path(root);
    if !path.is_file() {
        return Finding::new(
            "log_evidence",
            Status::NotObserved,
            format!(
                "{} not found; no local hook-activity evidence yet. After a real host tool call, inspect `phr-mcp stats` and the host result.",
                path.display()
            ),
        );
    }
    let opts = crate::action_log::ReadOpts {
        limit: Some(20),
        kind: Some("hook".to_string()),
        ..Default::default()
    };
    match crate::action_log::read_recent(&path, &opts) {
        Ok(entries) => match entries.last() {
            Some(entry) => Finding::new(
                "log_evidence",
                Status::NotObserved,
                format!(
                    "local log evidence only (not proof the host currently runs these hooks): last `{}` at ts={} in {}.",
                    entry.event,
                    entry.ts,
                    path.display()
                ),
            ),
            None => Finding::new(
                "log_evidence",
                Status::NotObserved,
                format!(
                    "{} has no recorded hook events yet. Observe a real host tool call and inspect `phr-mcp stats`.",
                    path.display()
                ),
            ),
        },
        Err(e) => Finding::new(
            "log_evidence",
            Status::NotObserved,
            format!("{} could not be read: {e}", path.display()),
        ),
    }
}

pub fn render_table(report: &DoctorReport) {
    println!("doctor: host={} path={}", report.host, report.path);
    for f in &report.findings {
        println!(
            "  [{:<12}] {:<22} {}",
            status_label(f.status),
            f.id,
            f.message
        );
    }
    if let Some(probe) = &report.probe {
        probe::render_table(probe);
    }
}

fn status_label(status: Status) -> &'static str {
    match status {
        Status::Ok => "OK",
        Status::Warn => "WARN",
        Status::Fail => "FAIL",
        Status::NotObserved => "NOT_OBSERVED",
    }
}

#[cfg(test)]
mod tests;
