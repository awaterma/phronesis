//! Fact-assertion helpers used by the pre- and post-check hooks.
//!
//! Lifted out of `hook.rs` for focus: this module owns the translation
//! from "we have file content / a tool name / a project root" to "the
//! RETE network has the facts a rule's conditions can match against."
//! No I/O orchestration here; that stays in `hook.rs`.

use std::collections::HashSet;
use std::path::Path;

use phr::{Fact, ReteNetwork, Rule};

use crate::diff_extract;
use crate::fact_id::fact_id;
use crate::hook::HookError;
use crate::syntax;

/// Assert language-pack diagnostics from the content currently being checked.
/// Structural relations still come from the durable graph; this live producer
/// covers proposed pre-check content that has not reached disk yet.
pub(crate) async fn assert_language_pack_facts(
    network: &ReteNetwork,
    file_path: &str,
    content: &str,
) -> Result<(), HookError> {
    use crate::graph::extract::Extracted;
    use crate::graph::unit::{LANG_HELM3, UnitContext};

    let unit = crate::graph::unit::lang_of_path(file_path)
        .map(UnitContext::unnamed_for)
        .unwrap_or_default();
    let extracted = if file_path.ends_with(".lua") {
        crate::graph::lua::extract_lua(file_path, content, &unit)
    } else if file_path.ends_with(".json") {
        crate::graph::json_extractor::extract_json(file_path, content, &unit)
    } else if file_path.ends_with(".yaml") || file_path.ends_with(".yml") {
        if content.contains("{{") && file_path.contains("/templates/") {
            let chart_root = file_path.split_once("/templates/").map(|(root, _)| root);
            crate::graph::helm3::extract_helm3(
                file_path,
                content,
                &UnitContext::unnamed_for(LANG_HELM3),
                chart_root,
            )
        } else {
            crate::graph::yaml::extract_yaml(file_path, content, &unit)
        }
    } else if file_path.ends_with(".tpl") {
        let chart_root = file_path.split_once("/templates/").map(|(root, _)| root);
        crate::graph::helm3::extract_helm3(
            file_path,
            content,
            &UnitContext::unnamed_for(LANG_HELM3),
            chart_root,
        )
    } else {
        Extracted::default()
    };

    const DIAGNOSTICS: &[&str] = &[
        "lua_dynamic_code_load",
        "json_schema_unknown_dialect",
        "yaml_duplicate_key",
        "yaml_undefined_alias",
        "yaml_merge_key",
        "helm3_dynamic_tpl",
        "helm3_cluster_lookup",
    ];
    for edge in extracted
        .edges
        .into_iter()
        .filter(|edge| DIAGNOSTICS.contains(&edge.p.as_str()))
    {
        network.assert_fact(edge.to_fact()).await?;
    }
    Ok(())
}

pub(crate) async fn assert_diff_facts(
    network: &ReteNetwork,
    file_path: &str,
    old: Option<&str>,
    new: &str,
) -> Result<(), HookError> {
    let facts = diff_extract::extract(file_path, old, new);

    for (predicate, items) in [
        ("function_added", &facts.functions_added),
        ("function_removed", &facts.functions_removed),
        ("import_added", &facts.imports_added),
        ("import_removed", &facts.imports_removed),
    ] {
        for (i, item) in items.iter().enumerate() {
            network
                .assert_fact(Fact {
                    id: fact_id(predicate, &[file_path, item, &i.to_string()]),
                    predicate: predicate.to_string(),
                    args: vec![file_path.to_string(), item.clone()],
                    timestamp: 0,
                    source: Some(format!("diff:{predicate}")),
                })
                .await?;
        }
    }
    Ok(())
}

/// Filter heavy-clone counts to only entries that are new or have increased.
///
/// `new` is the list of `(fn_name, count)` pairs extracted from the
/// post-edit content. `old`, when `Some`, is the same list extracted from
/// the prior content; when `None`, no filtering is applied.
///
/// Returns only entries where the function did not exist in `old` (implicit
/// count of 0) or its count strictly exceeds the matching old entry's count.
/// A decreased count is suppressed — the edit improved things, even if the
/// fn is still heavy.
pub(crate) fn filter_new_or_increased_clone_counts(
    new: &[(String, usize)],
    old: Option<&[(String, usize)]>,
) -> Vec<(String, usize)> {
    let Some(old_counts) = old else {
        return new.to_vec();
    };
    new.iter()
        .filter(|(fn_name, new_count)| {
            let old_count = old_counts
                .iter()
                .find(|(n, _)| n == fn_name)
                .map(|(_, c)| *c)
                .unwrap_or(0);
            *new_count > old_count
        })
        .cloned()
        .collect()
}

/// Run the values analyzer over the post-edit content and assert facts about
/// structural properties the diff extractor can't see. The set of predicates
/// emitted is whatever `SyntaxFacts::all_facts` produces; see
/// `src/values/facts.rs` for the canonical list.
pub(crate) async fn assert_values_facts(
    network: &ReteNetwork,
    file_path: &str,
    content: &str,
    old_content: Option<&str>,
) -> Result<(), HookError> {
    // Production-only predicates run against test-stripped content so rules
    // like `function_returns_result_string` don't fire on inline test code.
    let production = diff_extract::strip_test_blocks(file_path, content);
    let prod_facts = syntax::extract(file_path, &production);

    // Test-quality predicates need the unstripped content so they can see
    // `#[test] fn` bodies that strip_test_blocks would otherwise remove.
    let unstripped = syntax::extract(file_path, content);

    // Merge: take production predicates from `prod_facts`, take test-quality
    // predicates from `unstripped`. Currently only `tests_without_assertion`
    // belongs to the test-quality group.
    let mut facts = prod_facts;
    facts.tests_without_assertion = unstripped.tests_without_assertion;

    // Delta filter: warn-clone-heavy should only fire when a heavy-clone
    // function is newly added or its count increased compared to prior
    // content. Otherwise the rule re-fires every edit to a file with a
    // long-standing heavy function. `old_content` is only available at
    // pre-check (post-check disk already has new content).
    if let Some(old) = old_content {
        let old_stripped = diff_extract::strip_test_blocks(file_path, old);
        let old_facts = syntax::extract(file_path, &old_stripped);
        facts.function_clone_counts_high = filter_new_or_increased_clone_counts(
            &facts.function_clone_counts_high,
            Some(&old_facts.function_clone_counts),
        );
    }

    for fact in facts.all_facts(file_path) {
        network.assert_fact(fact).await?;
    }
    Ok(())
}

/// Assert `test_exists_for(name)` or `no_test_for(name)` per function name.
///
/// Heuristic search:
/// 1. The source file itself (inline `#[test]` / `def test_X` patterns)
/// 2. Conventional sibling test paths (`<stem>_test.<ext>`, `tests/<stem>_test.<ext>`)
///
/// A function is "tested" if its name appears anywhere in any of the candidate
/// test bodies. This is intentionally permissive — false positives are safer
/// than blocking a legitimate edit because the test exists but doesn't match
/// our regex.
pub(crate) async fn assert_test_facts(
    network: &ReteNetwork,
    project_root: &Path,
    file_path: &str,
    function_names: &[String],
) -> Result<(), HookError> {
    if function_names.is_empty() {
        return Ok(());
    }
    let candidates = test_candidate_paths(project_root, file_path);
    let mut test_bodies: Vec<String> = Vec::with_capacity(candidates.len());
    for p in &candidates {
        if let Ok(body) = tokio::fs::read_to_string(p).await {
            test_bodies.push(body);
        }
    }

    for (i, name) in function_names.iter().enumerate() {
        let has_test = test_bodies.iter().any(|body| body.contains(name.as_str()));
        let predicate = if has_test {
            "test_exists_for"
        } else {
            "no_test_for"
        };
        network
            .assert_fact(Fact {
                id: fact_id(predicate, &[name, &i.to_string()]),
                predicate: predicate.to_string(),
                args: vec![name.clone()],
                timestamp: 0,
                source: Some("hook".to_string()),
            })
            .await?;
    }
    Ok(())
}

fn test_candidate_paths(project_root: &Path, file_path: &str) -> Vec<std::path::PathBuf> {
    let path = Path::new(file_path);
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("rs");

    let mut candidates = vec![
        // The file itself (Rust inline #[cfg(test)] mod tests, Python def test_…)
        project_root.join(file_path),
        // Sibling test files
        path.parent()
            .unwrap_or(Path::new(""))
            .join(format!("{}_test.{}", stem, ext)),
        path.parent()
            .unwrap_or(Path::new(""))
            .join(format!("test_{}.{}", stem, ext)),
        // tests/ directory at project root
        project_root
            .join("tests")
            .join(format!("{}_test.{}", stem, ext)),
        project_root
            .join("tests")
            .join(format!("test_{}.{}", stem, ext)),
        project_root.join("tests").join(format!("{}.{}", stem, ext)),
    ];

    // Resolve relative paths against project root
    candidates.iter_mut().for_each(|p| {
        if p.is_relative() {
            *p = project_root.join(&*p);
        }
    });
    candidates.retain(|p| p.exists());
    candidates
}

pub(crate) async fn assert_common_facts(
    network: &ReteNetwork,
    file_path: &str,
    tool_name: &str,
    phase: &str,
) -> Result<(), HookError> {
    let facts = vec![
        Fact {
            id: "file_path".to_string(),
            predicate: "file_path".to_string(),
            args: vec![file_path.to_string()],
            timestamp: 0,
            source: Some("hook".to_string()),
        },
        Fact {
            id: "hook_phase".to_string(),
            predicate: "hook_phase".to_string(),
            args: vec![phase.to_string()],
            timestamp: 0,
            source: Some("hook".to_string()),
        },
        Fact {
            id: "change_type".to_string(),
            predicate: "change_type".to_string(),
            args: vec![tool_name.to_lowercase()],
            timestamp: 0,
            source: Some("hook".to_string()),
        },
    ];

    for part in file_path.split('/') {
        if !part.is_empty() {
            network
                .assert_fact(Fact {
                    id: fact_id("file_path_matches", &[part]),
                    predicate: "file_path_matches".to_string(),
                    args: vec![part.to_string()],
                    timestamp: 0,
                    source: Some("hook".to_string()),
                })
                .await?;
        }
    }

    // Extension fact for rules that need to scope by file type.
    // Emitted once per file as `file_extension_is("rs")`, `file_extension_is("rhai")`, etc.
    if let Some(ext) = file_path
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
    {
        network
            .assert_fact(Fact {
                id: fact_id("file_extension_is", &[&ext]),
                predicate: "file_extension_is".to_string(),
                args: vec![ext],
                timestamp: 0,
                source: Some("hook".to_string()),
            })
            .await?;
    }

    for fact in facts {
        network.assert_fact(fact).await?;
    }
    for fact in project_path_facts(&crate::security::project_root(), file_path) {
        network.assert_fact(fact).await?;
    }

    // Clock facts (business-hours-local, weekday-local, hour-local) — let
    // rules condition on when the hook is firing. Cheap; read the local
    // clock once per invocation.
    for cf in crate::clock_facts::now() {
        let args: Vec<&str> = cf.args.iter().map(String::as_str).collect();
        let id = fact_id(cf.predicate, &args);
        network
            .assert_fact(Fact {
                id,
                predicate: cf.predicate.to_string(),
                args: cf.args,
                timestamp: 0,
                source: Some("hook".to_string()),
            })
            .await?;
    }

    Ok(())
}

/// Root-anchored path facts: `project_path_is(<rel>)` for the file and
/// `project_path_under(<dir>)` for each ancestor directory inside the
/// project. `verification/templates/x.rhai` yields
/// `project_path_is("verification/templates/x.rhai")`,
/// `project_path_under("verification")`, and
/// `project_path_under("verification/templates")`.
///
/// Unlike the per-segment `file_path_matches`, these are anchored at the
/// project root and keep segment adjacency, so a rule can name one exact
/// location (the verification trust anchors) without matching lookalikes
/// nested elsewhere or a checkout that itself lives under such a directory.
/// The path is lexically normalized (`.`/`..`); the deepest existing
/// ancestor is also canonicalized, so a symlinked directory or a
/// `/var` vs `/private/var` spelling still resolves to where the write
/// lands. Both spellings are emitted. A path outside the root yields
/// nothing. A lowercase form is emitted too when it differs: the default
/// macOS filesystem is case-insensitive.
///
/// Known gap: a *dangling* symlink whose target is a not-yet-created anchor
/// (`ln -s .phronesis/verification.json x.json`, then Write `x.json`) is not
/// followed — `canonicalize` fails on it, so only the link's own path is
/// emitted.
pub(crate) fn project_path_facts(root: &Path, file_path: &str) -> Vec<Fact> {
    if file_path.is_empty() {
        return Vec::new();
    }
    let lexical = lexical_normalize(&root.join(file_path));
    let lexical_root = lexical_normalize(root);
    let canon_root = root.canonicalize().unwrap_or_else(|_| lexical_root.clone());
    let resolved = canonicalize_existing_prefix(&lexical);

    let mut rels: Vec<String> = Vec::new();
    for (path, base) in [
        (&lexical, &lexical_root),
        (&lexical, &canon_root),
        (&resolved, &canon_root),
    ] {
        if let Ok(rel) = path.strip_prefix(base) {
            let parts: Vec<String> = rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect();
            if parts.is_empty() {
                continue;
            }
            let joined = parts.join("/");
            let lower = joined.to_lowercase();
            rels.push(joined);
            rels.push(lower);
        }
    }

    let mut seen = HashSet::new();
    let mut facts = Vec::new();
    let mut push = |predicate: &str, arg: String| {
        if seen.insert((predicate.to_string(), arg.clone())) {
            facts.push(Fact {
                id: format!("{predicate}_{arg}"),
                predicate: predicate.to_string(),
                args: vec![arg],
                timestamp: 0,
                source: Some("hook".to_string()),
            });
        }
    };
    for rel in rels {
        let mut dir = String::new();
        let mut parts = rel.split('/').peekable();
        while let Some(part) = parts.next() {
            if parts.peek().is_none() {
                break;
            }
            if !dir.is_empty() {
                dir.push('/');
            }
            dir.push_str(part);
            push("project_path_under", dir.clone());
        }
        push("project_path_is", rel);
    }
    facts
}

/// Resolve `.` and `..` without touching the filesystem.
fn lexical_normalize(path: &Path) -> std::path::PathBuf {
    use std::path::Component;
    let mut out = std::path::PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push(c);
                }
            }
            other => out.push(other),
        }
    }
    out
}

/// Canonicalize the deepest ancestor of `path` that exists and re-attach
/// the not-yet-created tail (the file a Write is about to create).
fn canonicalize_existing_prefix(path: &Path) -> std::path::PathBuf {
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    let mut cur = path;
    loop {
        if let Ok(mut canon) = cur.canonicalize() {
            for part in tail.iter().rev() {
                canon.push(part);
            }
            return canon;
        }
        match (cur.parent(), cur.file_name()) {
            (Some(parent), Some(name)) => {
                tail.push(name.to_os_string());
                cur = parent;
            }
            _ => return path.to_path_buf(),
        }
    }
}

pub(crate) async fn check_content_patterns(
    network: &ReteNetwork,
    file_path: &str,
    content: &str,
    patterns: &[String],
) -> Result<(), HookError> {
    // Strip out test-scoped regions so patterns inside `#[cfg(test)]` blocks
    // or `#[test] fn` bodies don't fire production-only rules. For unsupported
    // languages this is an identity no-op.
    let production = diff_extract::strip_test_blocks(file_path, content);

    for pattern in patterns {
        if production.contains(pattern.as_str()) {
            network
                .assert_fact(Fact {
                    id: fact_id("new_content_contains", &[pattern]),
                    predicate: "new_content_contains".to_string(),
                    args: vec![pattern.clone()],
                    timestamp: 0,
                    source: Some("hook".to_string()),
                })
                .await?;
        }
    }
    Ok(())
}

/// Predicate: a regex over the raw command text.
pub(crate) const BASH_COMMAND_MATCHES: &str = "bash_command_matches";
/// Predicate: a regex over the command's code — `shell_code_text` — with
/// heredoc bodies removed and `sh -c` scripts unwrapped.
pub(crate) const BASH_COMMAND_CODE_MATCHES: &str = "bash_command_code_matches";

/// One command-text regex a rule conditions on, and which text it runs on.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct BashCommandPattern {
    pub(crate) predicate: &'static str,
    pub(crate) pattern: String,
}

/// For each command regex that matches, assert a fact carrying the pattern
/// so it alpha-matches the rule's condition arg. `bash_command_matches`
/// runs on the raw command; `bash_command_code_matches` on
/// `shell_code_text(command)`, computed once and only when needed.
/// Callers gate this to command tools (Bash / run_shell_command): the
/// predicate is about the command being run, never about file content
/// that happens to quote the same text.
///
/// An invalid regex is skipped with a stderr warning — a rule-author typo
/// must never block the project.
pub(crate) async fn check_bash_command_patterns(
    network: &ReteNetwork,
    command: &str,
    patterns: &[BashCommandPattern],
) -> Result<(), HookError> {
    let mut code: Option<String> = None;
    for BashCommandPattern { predicate, pattern } in patterns {
        let re = match regex::Regex::new(pattern) {
            Ok(re) => re,
            Err(e) => {
                eprintln!(
                    "phronesis: WARNING — invalid {} regex '{}': {}",
                    predicate, pattern, e
                );
                continue;
            }
        };
        let text = if *predicate == BASH_COMMAND_CODE_MATCHES {
            code.get_or_insert_with(|| shell_code_text(command))
                .as_str()
        } else {
            command
        };
        if re.is_match(text) {
            network
                .assert_fact(Fact {
                    id: fact_id(predicate, &[pattern.as_str()]),
                    predicate: predicate.to_string(),
                    args: vec![pattern.clone()],
                    timestamp: 0,
                    source: Some("hook".to_string()),
                })
                .await?;
        }
    }
    Ok(())
}

/// Collect every distinct `args[0]` from rules' `bash_command_matches` and
/// `bash_command_code_matches` conditions — the regex set the hook
/// evaluates against command text.
pub(crate) fn collect_bash_command_patterns(rules: &[Rule]) -> Vec<BashCommandPattern> {
    let mut seen = std::collections::HashSet::new();
    rules
        .iter()
        .flat_map(|r| &r.conditions)
        .filter_map(|c| {
            let predicate = match c.predicate.as_str() {
                BASH_COMMAND_MATCHES => BASH_COMMAND_MATCHES,
                BASH_COMMAND_CODE_MATCHES => BASH_COMMAND_CODE_MATCHES,
                _ => return None,
            };
            let pattern = c.args.first()?.clone();
            Some(BashCommandPattern { predicate, pattern })
        })
        .filter(|p| seen.insert(p.clone()))
        .collect()
}

/// Shells whose heredoc body, or `-c` script, is code rather than data.
const SHELLS: &[&str] = &["bash", "sh", "zsh", "dash", "ksh"];

/// The command as code, for `bash_command_code_matches`.
///
/// - Heredoc bodies are removed: a `git commit -F - <<'EOF'` message or a
///   `cat <<EOF > doc.md` document is data, not a command. The exception is
///   a heredoc fed to a shell (`bash <<EOF`, `cat <<EOF | sh`), whose body
///   is code and is kept.
/// - The quoted script of `bash -c '…'` / `sh -c "…"` / `eval "…"` is
///   appended as its own line (nested up to three levels), so a command run
///   through a shell is matched as a command rather than skipped as a
///   quoted string.
///
/// Lexical, like every shell predicate: it approximates the shell's own
/// parse and is only as strong as an advisory rule needs.
pub(crate) fn shell_code_text(command: &str) -> String {
    let stripped = strip_heredoc_bodies(command);
    let mut out = stripped.clone();
    let mut frontier = vec![stripped];
    for _ in 0..3 {
        let mut next = Vec::new();
        for text in &frontier {
            for script in shell_c_scripts(text) {
                let inner = strip_heredoc_bodies(&script);
                out.push('\n');
                out.push_str(&inner);
                next.push(inner);
            }
        }
        if next.is_empty() {
            break;
        }
        frontier = next;
    }
    out
}

/// The quoted script arguments of `sh -c` / `bash -lc` / `eval` in `text`.
fn shell_c_scripts(text: &str) -> Vec<String> {
    static SHELL_C: std::sync::LazyLock<Option<regex::Regex>> = std::sync::LazyLock::new(|| {
        regex::Regex::new(
            r#"\b(?:(?:bash|sh|zsh|dash|ksh)(?:\s+-[A-Za-z]+)*?\s+-[A-Za-z]*c[A-Za-z]*|eval)\s+(?:'([^']*)'|"((?:[^"\\]|\\.)*)")"#,
        )
        .ok()
    });
    let Some(re) = SHELL_C.as_ref() else {
        return Vec::new();
    };
    re.captures_iter(text)
        .filter_map(|c| {
            if let Some(single) = c.get(1) {
                return Some(single.as_str().to_string());
            }
            // Double-quoted: undo the escapes the shell would remove.
            let raw = c.get(2)?.as_str();
            let mut out = String::with_capacity(raw.len());
            let mut chars = raw.chars().peekable();
            while let Some(ch) = chars.next() {
                if ch == '\\'
                    && let Some(&next) = chars.peek()
                    && matches!(next, '"' | '\\' | '$' | '`')
                {
                    out.push(next);
                    chars.next();
                    continue;
                }
                out.push(ch);
            }
            Some(out)
        })
        .collect()
}

/// A heredoc whose body follows the current line.
struct PendingHeredoc {
    delimiter: String,
    strip_tabs: bool,
    keep_body: bool,
}

/// Remove heredoc bodies (and their terminator lines) from `command`,
/// keeping the bodies of heredocs fed to a shell. `<<` inside quotes, a
/// comment, or a `<<<` here-string starts nothing.
fn strip_heredoc_bodies(command: &str) -> String {
    let mut out = String::with_capacity(command.len());
    let mut pending: std::collections::VecDeque<PendingHeredoc> = Default::default();
    let mut current: Option<PendingHeredoc> = None;
    // Quote state carries across lines: a quoted string may span several.
    let mut quote: Option<char> = None;

    for line in command.split_inclusive('\n') {
        if let Some(doc) = &current {
            let content = line.trim_end_matches(['\n', '\r']);
            let content = if doc.strip_tabs {
                content.trim_start_matches('\t')
            } else {
                content
            };
            let keep = doc.keep_body;
            if content == doc.delimiter {
                current = pending.pop_front();
            }
            if keep {
                out.push_str(line);
            }
            continue;
        }
        out.push_str(line);
        scan_line_for_heredocs(line, &mut quote, &mut pending);
        if current.is_none() {
            current = pending.pop_front();
        }
    }
    out
}

/// Scan one command line (outside any heredoc body) for heredoc operators,
/// updating the carried quote state.
fn scan_line_for_heredocs(
    line: &str,
    quote: &mut Option<char>,
    pending: &mut std::collections::VecDeque<PendingHeredoc>,
) {
    let chars: Vec<char> = line.chars().collect();
    let mut segment_start = 0;
    let mut i = 0;
    // Depth of open arithmetic contexts (`$(( … ))` or standalone `(( … ))`).
    // Bash never nests two independent subshells without a space between
    // their parens, so a bare `((` is always the arithmetic-evaluation
    // idiom; inside it `<<`/`<<=` are shift operators, not heredoc starts.
    let mut arith_depth: u32 = 0;
    while i < chars.len() {
        let c = chars[i];
        match *quote {
            Some('\'') => {
                if c == '\'' {
                    *quote = None;
                }
                i += 1;
                continue;
            }
            Some(_) => {
                if c == '\\' {
                    i += 2;
                    continue;
                }
                if c == '"' {
                    *quote = None;
                }
                i += 1;
                continue;
            }
            None => {}
        }
        if arith_depth > 0 {
            match c {
                '(' => arith_depth += 1,
                ')' => arith_depth -= 1,
                _ => {}
            }
            i += 1;
            continue;
        }
        if c == '(' && chars.get(i + 1) == Some(&'(') {
            arith_depth = 2;
            i += 2;
            continue;
        }
        match c {
            '\\' => {
                i += 2;
                continue;
            }
            '\'' | '"' => *quote = Some(c),
            '#' if i == 0 || chars[i - 1].is_whitespace() => return,
            ';' | '&' | '|' | '(' | '`' => segment_start = i + 1,
            '<' if chars.get(i + 1) == Some(&'<') => {
                if chars.get(i + 2) == Some(&'<') {
                    i += 3;
                    continue;
                }
                let mut j = i + 2;
                let strip_tabs = chars.get(j) == Some(&'-');
                if strip_tabs {
                    j += 1;
                }
                while matches!(chars.get(j), Some(' ' | '\t')) {
                    j += 1;
                }
                let mut delimiter = String::new();
                if let Some(&q @ ('\'' | '"')) = chars.get(j) {
                    j += 1;
                    while let Some(&d) = chars.get(j) {
                        j += 1;
                        if d == q {
                            break;
                        }
                        delimiter.push(d);
                    }
                } else {
                    while let Some(&d) = chars.get(j) {
                        if d.is_whitespace() || ";&|<>()".contains(d) {
                            break;
                        }
                        if d != '\\' {
                            delimiter.push(d);
                        }
                        j += 1;
                    }
                }
                if !delimiter.is_empty() {
                    let segment: String = chars[segment_start..i].iter().collect();
                    let rest: String = chars[j..].iter().collect();
                    pending.push_back(PendingHeredoc {
                        delimiter,
                        strip_tabs,
                        keep_body: runs_a_shell(&segment) || pipes_into_a_shell(&rest),
                    });
                }
                i = j;
                continue;
            }
            _ => {}
        }
        i += 1;
    }
}

/// Whether a command segment's program is a shell (after env assignments
/// and common wrappers such as `sudo`/`env`).
fn runs_a_shell(segment: &str) -> bool {
    segment
        .split_whitespace()
        .find(|tok| {
            let assignment = tok.split_once('=').is_some_and(|(name, _)| {
                !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            });
            !(assignment
                || tok.starts_with('-')
                || matches!(*tok, "sudo" | "env" | "command" | "exec" | "nohup" | "time"))
        })
        .map(|prog| prog.rsplit('/').next().unwrap_or(prog))
        .is_some_and(|prog| SHELLS.contains(&prog))
}

/// Whether the rest of a heredoc's line pipes its output into a shell.
fn pipes_into_a_shell(rest: &str) -> bool {
    rest.split('|').skip(1).any(runs_a_shell)
}

/// FNV-1a 64 over joined args — stable fact IDs so re-asserting the same
/// coverage fact replaces it instead of accumulating (the graph `fact_id()`
/// precedent). 12 hex chars keep IDs readable.
fn coverage_args_hash(args: &[String]) -> String {
    let joined = args.join("\u{1f}");
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in joined.as_bytes() {
        hash ^= u64::from(*b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:012x}")
}

/// Every predicate mentioned by any loaded rule's conditions. The coverage
/// hydration demand-gates on this set: facts assert only when a loaded rule
/// mentions a coverage relation (the `graph/hydrate.rs` precedent — a
/// footprint bounded by construction, spec acceptance A4).
pub(crate) fn collect_rule_predicates(rules: &[Rule]) -> HashSet<String> {
    rules
        .iter()
        .flat_map(|r| r.conditions.iter().map(|c| c.predicate.clone()))
        .collect()
}

/// Hydrate coverage-evidence facts into the network for this event.
///
/// Demand-gated twice: the hydrate module gates on `rule_relations` itself,
/// and this wrapper skips the whole pipeline (store read, git probe) when no
/// loaded rule mentions any coverage relation. A corrupt coverage store is
/// NOT a hydration failure: it warns on stderr, asserts
/// `store_corrupt(coverage, <reason>)` when a rule mentions it (so a rule can
/// block on it), and gap facts are derived as if no evidence existed. Only
/// failures outside the store (region mapping) fail open with a warning.
/// `PHRONESIS_NO_COVERAGE` disables the whole path.
pub(crate) async fn assert_coverage_facts(
    network: &ReteNetwork,
    project_root: &Path,
    rule_predicates: &HashSet<String>,
    edited: &[(String, Option<String>, String)],
) -> Result<(), HookError> {
    use crate::coverage::hydrate::{EditedFile, HydrationInput, RELATIONS, hydrate};

    if std::env::var_os("PHRONESIS_NO_COVERAGE").is_some() {
        return Ok(());
    }
    // Demand gate (spec A4): no coverage relation in any loaded rule -> zero
    // hydration work.
    if !RELATIONS.iter().any(|r| rule_predicates.contains(*r)) {
        return Ok(());
    }

    let edited_files: Vec<EditedFile> = edited
        .iter()
        .map(|(path, old, new)| EditedFile {
            path: path.clone(),
            old: old.as_deref(),
            new,
        })
        .collect();
    let head_sha = match crate::lifecycle::outcome::git_head_probe(project_root) {
        crate::lifecycle::outcome::HeadProbe::Head(sha) => Some(sha),
        _ => None,
    };
    let input = HydrationInput {
        root: project_root,
        rule_relations: rule_predicates.clone(),
        edited: edited_files,
        head_sha,
    };

    let facts = match hydrate(&input) {
        Ok(h) => {
            if let Some(c) = &h.store_corrupt {
                eprintln!(
                    "phronesis: WARNING — {c}; coverage evidence ignored (gaps reported as untested). {}.",
                    crate::coverage::store::RECOLLECT_HINT
                );
            }
            if h.store_busy {
                eprintln!(
                    "phronesis: note — coverage store is being replaced by an import in progress; evidence treated as stale for this event."
                );
            }
            h.facts
        }
        Err(e) => {
            eprintln!("phronesis: WARNING — coverage hydration failed: {}", e);
            return Ok(());
        }
    };

    for f in facts {
        let hash = coverage_args_hash(&f.args);
        let fact = Fact {
            id: format!("coverage:{}:{hash}", f.predicate),
            predicate: f.predicate.clone(),
            args: f.args,
            timestamp: 0,
            source: Some("coverage".to_string()),
        };
        if let Err(e) = network.assert_fact(fact).await {
            eprintln!("phronesis: WARNING — coverage fact rejected: {}", e);
        }
    }
    Ok(())
}

/// Hydrate property-ontology facts (SPEC-property-ontology.md) — the
/// coverage hydration pattern: demand-gated on rule predicates,
/// `PHRONESIS_NO_PROPERTIES` opt-out. A corrupt property store (properties.json
/// or property-results.jsonl) is NOT a hydration failure: it warns on
/// stderr, asserts `store_corrupt(properties, <reason>)` when a rule mentions
/// it, and the rest is derived as if the corrupt file held no evidence, so
/// obligations still fire (D8).
pub(crate) async fn assert_properties_facts(
    network: &ReteNetwork,
    project_root: &Path,
    rule_predicates: &HashSet<String>,
    edited: &[(String, Option<String>, String)],
) -> Result<(), HookError> {
    use crate::properties::hydrate::{EditedFile, PropertyHydrationInput, RELATIONS, hydrate};

    if std::env::var_os("PHRONESIS_NO_PROPERTIES").is_some() {
        return Ok(());
    }
    if !RELATIONS.iter().any(|r| rule_predicates.contains(*r)) {
        return Ok(());
    }
    let edited_files: Vec<EditedFile> = edited
        .iter()
        .map(|(path, old, new)| EditedFile {
            path: path.clone(),
            old: old.as_deref(),
            new,
        })
        .collect();
    let head_sha = match crate::lifecycle::outcome::git_head_probe(project_root) {
        crate::lifecycle::outcome::HeadProbe::Head(sha) => Some(sha),
        _ => None,
    };
    let input = PropertyHydrationInput {
        root: project_root,
        rule_relations: rule_predicates.clone(),
        edited: edited_files,
        head_sha,
    };
    let facts = match hydrate(&input) {
        Ok(h) => {
            if let Some(c) = &h.store_corrupt {
                eprintln!(
                    "phronesis: WARNING — property store corrupt ({}): {c}; property evidence ignored (obligations reported as unproved).",
                    c.reason()
                );
            }
            h.facts
        }
        Err(e) => {
            eprintln!("phronesis: WARNING — property hydration failed: {}", e);
            return Ok(());
        }
    };
    for f in facts {
        let joined = f.args.join("\u{1f}");
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for b in joined.as_bytes() {
            hash ^= u64::from(*b);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        let fact = Fact {
            id: format!("property:{}:{hash:012x}", f.predicate),
            predicate: f.predicate.clone(),
            args: f.args,
            timestamp: 0,
            source: Some("properties".to_string()),
        };
        if let Err(e) = network.assert_fact(fact).await {
            eprintln!("phronesis: WARNING — property fact rejected: {}", e);
        }
    }
    Ok(())
}

/// Collect every distinct `args[0]` from rules' `new_content_contains`
/// conditions. The hook scans for exactly these patterns; any rule whose
/// condition references a pattern automatically gets it checked.
pub(crate) fn collect_content_patterns(rules: &[Rule]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    rules
        .iter()
        .flat_map(|r| &r.conditions)
        .filter(|c| c.predicate == "new_content_contains")
        .filter_map(|c| c.args.first())
        .filter(|s| seen.insert((*s).clone()))
        .cloned()
        .collect()
}

/// Same for `file_missing_pattern` — used by post-check.
pub(crate) fn collect_missing_patterns(rules: &[Rule]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    rules
        .iter()
        .flat_map(|r| &r.conditions)
        .filter(|c| c.predicate == "file_missing_pattern")
        .filter_map(|c| c.args.first())
        .filter(|s| seen.insert((*s).clone()))
        .cloned()
        .collect()
}

pub(crate) async fn check_missing_patterns(
    network: &ReteNetwork,
    content: &str,
    patterns: &[String],
) -> Result<(), HookError> {
    for pattern in patterns {
        if !content.contains(pattern.as_str()) {
            network
                .assert_fact(Fact {
                    id: fact_id("file_missing_pattern", &[pattern]),
                    predicate: "file_missing_pattern".to_string(),
                    args: vec![pattern.clone()],
                    timestamp: 0,
                    source: Some("hook".to_string()),
                })
                .await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod project_path_tests {
    use super::*;

    fn has(facts: &[Fact], predicate: &str, arg: &str) -> bool {
        facts
            .iter()
            .any(|f| f.predicate == predicate && f.args == [arg.to_string()])
    }

    #[test]
    fn relative_and_absolute_paths_anchor_at_the_root() {
        let d = tempfile::tempdir().unwrap();
        let abs = d.path().join("verification/templates/h.rhai");
        for p in [
            "verification/templates/h.rhai".to_string(),
            "./verification/x/../templates/h.rhai".to_string(),
            abs.display().to_string(),
        ] {
            let facts = project_path_facts(d.path(), &p);
            assert!(
                has(&facts, "project_path_is", "verification/templates/h.rhai"),
                "{p}"
            );
            assert!(has(&facts, "project_path_under", "verification"), "{p}");
            assert!(
                has(&facts, "project_path_under", "verification/templates"),
                "{p}"
            );
        }
    }

    #[test]
    fn nested_lookalikes_and_outside_paths_do_not_anchor() {
        let d = tempfile::tempdir().unwrap();
        let facts = project_path_facts(d.path(), "src/verification/templates/x.html");
        assert!(!has(&facts, "project_path_under", "verification/templates"));
        let outside = project_path_facts(d.path(), "/etc/verification/templates/x");
        assert!(outside.is_empty(), "{outside:?}");
        assert!(project_path_facts(d.path(), "").is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_directory_resolves_to_where_the_write_lands() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("verification/templates")).unwrap();
        std::os::unix::fs::symlink(
            d.path().join("verification/templates"),
            d.path().join("tpl"),
        )
        .unwrap();
        let facts = project_path_facts(d.path(), "tpl/h.rhai");
        assert!(
            has(&facts, "project_path_under", "verification/templates"),
            "{facts:?}"
        );
    }

    #[test]
    fn a_case_variant_also_emits_the_lowercase_form() {
        let d = tempfile::tempdir().unwrap();
        let facts = project_path_facts(d.path(), ".Phronesis/Verification.json");
        assert!(has(
            &facts,
            "project_path_is",
            ".phronesis/verification.json"
        ));
    }
}

#[cfg(test)]
mod shell_code_tests {
    use super::shell_code_text;

    #[test]
    fn heredoc_bodies_are_removed() {
        let cmd = "git commit -F - <<'EOF'\nsubject\n\n  rm .phronesis/verification.json\nEOF\necho after";
        let code = shell_code_text(cmd);
        assert!(!code.contains("rm .phronesis"), "{code}");
        assert!(
            code.contains("git commit -F -") && code.contains("echo after"),
            "{code}"
        );
    }

    #[test]
    fn dash_heredocs_and_multiple_heredocs_on_one_line() {
        let cmd = "cat <<-A <<B > out\n\tbody a\n\tA\nbody b\nB\nnext";
        let code = shell_code_text(cmd);
        assert!(!code.contains("body"), "{code}");
        assert!(code.ends_with("next"), "{code}");
    }

    #[test]
    fn a_heredoc_fed_to_a_shell_keeps_its_body() {
        for cmd in [
            "bash <<'EOF'\necho x > f\nEOF",
            "sudo sh -s <<EOF\necho x > f\nEOF",
            "cat <<EOF | bash\necho x > f\nEOF",
        ] {
            assert!(shell_code_text(cmd).contains("echo x > f"), "{cmd}");
        }
    }

    #[test]
    fn quoted_or_commented_heredoc_markers_and_here_strings_start_nothing() {
        for cmd in [
            "git commit -m \"use <<EOF\"\nrm f",
            "echo hi # <<EOF\nrm f",
            "grep x <<< \"$v\"\nrm f",
        ] {
            assert!(shell_code_text(cmd).contains("rm f"), "{cmd}");
        }
    }

    #[test]
    fn arithmetic_left_shift_is_not_a_heredoc() {
        let code = shell_code_text("x=$((1<<2))\nrm .phronesis/verification.json");
        assert!(code.contains("rm .phronesis/verification.json"), "{code}");
        let code = shell_code_text("x=$(( a << b ))\nrm f");
        assert!(code.contains("rm f"), "{code}");
        let code = shell_code_text("(( x <<= 1 ))\nrm f");
        assert!(code.contains("rm f"), "{code}");
    }

    #[test]
    fn shell_c_scripts_are_unwrapped() {
        let code = shell_code_text("bash -c \"echo \\\"x\\\" > a\" && sh -lc 'rm b'");
        assert!(code.contains("\necho \"x\" > a"), "{code}");
        assert!(code.contains("\nrm b"), "{code}");
        let nested = shell_code_text("sh -c \"bash -c 'rm c'\"");
        assert!(nested.contains("\nrm c"), "{nested}");
        // `ssh -c` is a cipher flag, not a shell.
        assert!(!shell_code_text("ssh -c aes host 'rm d'").contains("\nrm d"));
    }
}
