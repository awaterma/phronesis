//! Fact-assertion helpers used by the pre- and post-check hooks.
//!
//! Lifted out of `hook.rs` for focus: this module owns the translation
//! from "we have file content / a tool name / a project root" to "the
//! RETE network has the facts a rule's conditions can match against."
//! No I/O orchestration here; that stays in `hook.rs`.

use std::path::Path;

use phr::{Fact, ReteNetwork, Rule};

use crate::diff_extract;
use crate::hook::HookError;
use crate::syntax;

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
                    id: format!("{}_{}_{}", predicate, item, i),
                    predicate: predicate.to_string(),
                    args: vec![file_path.to_string(), item.clone()],
                    timestamp: 0,
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
    let test_bodies: Vec<String> = candidates
        .iter()
        .filter_map(|p| std::fs::read_to_string(p).ok())
        .collect();

    for (i, name) in function_names.iter().enumerate() {
        let has_test = test_bodies.iter().any(|body| body.contains(name.as_str()));
        let predicate = if has_test {
            "test_exists_for"
        } else {
            "no_test_for"
        };
        network
            .assert_fact(Fact {
                id: format!("{}_{}_{}", predicate, name, i),
                predicate: predicate.to_string(),
                args: vec![name.clone()],
                timestamp: 0,
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
        },
        Fact {
            id: "hook_phase".to_string(),
            predicate: "hook_phase".to_string(),
            args: vec![phase.to_string()],
            timestamp: 0,
        },
        Fact {
            id: "change_type".to_string(),
            predicate: "change_type".to_string(),
            args: vec![tool_name.to_lowercase()],
            timestamp: 0,
        },
    ];

    for part in file_path.split('/') {
        if !part.is_empty() {
            let fact_id = format!("file_path_matches_{}", part);
            network
                .assert_fact(Fact {
                    id: fact_id,
                    predicate: "file_path_matches".to_string(),
                    args: vec![part.to_string()],
                    timestamp: 0,
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
        let fact_id = format!("file_extension_is_{}", ext);
        network
            .assert_fact(Fact {
                id: fact_id,
                predicate: "file_extension_is".to_string(),
                args: vec![ext],
                timestamp: 0,
            })
            .await?;
    }

    for fact in facts {
        network.assert_fact(fact).await?;
    }

    // Clock facts (business-hours-local, weekday-local, hour-local) — let
    // rules condition on when the hook is firing. Cheap; read the local
    // clock once per invocation.
    for cf in crate::clock_facts::now() {
        let fact_id = format!("{}_{}", cf.predicate, cf.args.join("_"));
        network
            .assert_fact(Fact {
                id: fact_id,
                predicate: cf.predicate.to_string(),
                args: cf.args,
                timestamp: 0,
            })
            .await?;
    }

    Ok(())
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
            let fact_id = format!(
                "new_content_contains_{}",
                sanitize_fact_id_fragment(pattern)
            );
            network
                .assert_fact(Fact {
                    id: fact_id,
                    predicate: "new_content_contains".to_string(),
                    args: vec![pattern.clone()],
                    timestamp: 0,
                })
                .await?;
        }
    }
    Ok(())
}

/// For each `bash_command_matches` regex that matches `command`, assert a
/// fact carrying the pattern so it alpha-matches the rule's condition arg.
/// Callers gate this to command tools (Bash / run_shell_command): the
/// predicate is about the command being run, never about file content
/// that happens to quote the same text.
///
/// An invalid regex is skipped with a stderr warning — a rule-author typo
/// must never block the project.
pub(crate) async fn check_bash_command_patterns(
    network: &ReteNetwork,
    command: &str,
    patterns: &[String],
) -> Result<(), HookError> {
    for pattern in patterns {
        let re = match regex::Regex::new(pattern) {
            Ok(re) => re,
            Err(e) => {
                eprintln!(
                    "phronesis: WARNING — invalid bash_command_matches regex '{}': {}",
                    pattern, e
                );
                continue;
            }
        };
        if re.is_match(command) {
            let fact_id = format!(
                "bash_command_matches_{}",
                sanitize_fact_id_fragment(pattern)
            );
            network
                .assert_fact(Fact {
                    id: fact_id,
                    predicate: "bash_command_matches".to_string(),
                    args: vec![pattern.clone()],
                    timestamp: 0,
                })
                .await?;
        }
    }
    Ok(())
}

/// Collect every distinct `args[0]` from rules' `bash_command_matches`
/// conditions — the regex set the hook evaluates against command text.
pub(crate) fn collect_bash_command_patterns(rules: &[Rule]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    rules
        .iter()
        .flat_map(|r| &r.conditions)
        .filter(|c| c.predicate == "bash_command_matches")
        .filter_map(|c| c.args.first())
        .filter(|s| seen.insert((*s).clone()))
        .cloned()
        .collect()
}

/// Make a fact-id-safe fragment from an arbitrary pattern string. Whitespace
/// and any character that isn't ASCII alphanumeric becomes `_`. Stable for a
/// given input, but not necessarily reversible — IDs are opaque keys.
fn sanitize_fact_id_fragment(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
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
            let fact_id = format!(
                "file_missing_pattern_{}",
                sanitize_fact_id_fragment(pattern)
            );
            network
                .assert_fact(Fact {
                    id: fact_id,
                    predicate: "file_missing_pattern".to_string(),
                    args: vec![pattern.clone()],
                    timestamp: 0,
                })
                .await?;
        }
    }
    Ok(())
}
