//! Surface structural change facts from a code edit.
//!
//! When an LLM edits a source file, the hook needs predicates that describe
//! what the edit *did* — added a function, removed an import — not just what
//! substring it contains. This module produces a `DiffFacts` value from the
//! `(old_string, new_string)` pair of an `Edit` (or `(None, content)` for a
//! `Write`); the hook then asserts each item as a `function_added(file, name)`
//! / `function_removed(file, name)` / `import_added(file, target)` fact.
//!
//! v1 is regex-based per language. Tree-sitter would catch the cases this
//! misses (multi-line signatures, signatures spanning macros), and can be
//! plugged in behind the same `extract` function without changing callers.

use std::sync::LazyLock;

use regex::Regex;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct DiffFacts {
    pub functions_added: Vec<String>,
    pub functions_removed: Vec<String>,
    pub imports_added: Vec<String>,
    pub imports_removed: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Language {
    Rust,
    Python,
    JavaScript,
    Go,
}

fn language_of(file_path: &str) -> Option<Language> {
    let ext = file_path
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase());
    match ext.as_deref() {
        Some("rs") => Some(Language::Rust),
        Some("py" | "pyi") => Some(Language::Python),
        Some("ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs") => Some(Language::JavaScript),
        Some("go") => Some(Language::Go),
        _ => None,
    }
}

/// Extract diff facts from a source-file change.
///
/// `old` is `None` for a `Write` (no prior content) — in that case every
/// function and import in `new` is treated as added. `old` is `Some(_)` for
/// an `Edit`; set-difference produces added vs removed.
///
/// Unknown file extensions return an empty `DiffFacts` (rules using other
/// predicates still work).
pub fn extract(file_path: &str, old: Option<&str>, new: &str) -> DiffFacts {
    let Some(lang) = language_of(file_path) else {
        return DiffFacts::default();
    };

    let new_fns = functions(lang, new);
    let new_imports = imports(lang, new);

    match old {
        None => DiffFacts {
            functions_added: dedup_preserving_order(new_fns),
            functions_removed: vec![],
            imports_added: dedup_preserving_order(new_imports),
            imports_removed: vec![],
        },
        Some(old_text) => {
            let old_fns = functions(lang, old_text);
            let old_imports = imports(lang, old_text);
            DiffFacts {
                functions_added: set_diff(&new_fns, &old_fns),
                functions_removed: set_diff(&old_fns, &new_fns),
                imports_added: set_diff(&new_imports, &old_imports),
                imports_removed: set_diff(&old_imports, &new_imports),
            }
        }
    }
}

fn functions(lang: Language, text: &str) -> Vec<String> {
    match lang {
        Language::Rust => capture_all(&RUST_FN, text, 1),
        Language::Python => capture_all(&PY_FN, text, 1),
        Language::JavaScript => {
            let mut out = capture_all(&JS_FN_DECL, text, 1);
            out.extend(capture_all(&JS_FN_ASSIGN, text, 1));
            out.extend(capture_all(&JS_METHOD, text, 1));
            out
        }
        Language::Go => capture_all(&GO_FN, text, 1),
    }
}

fn imports(lang: Language, text: &str) -> Vec<String> {
    match lang {
        Language::Rust => capture_all(&RUST_USE, text, 1),
        Language::Python => {
            let mut out = capture_all(&PY_IMANAORT, text, 1);
            out.extend(capture_all(&PY_FROM, text, 1));
            out
        }
        Language::JavaScript => capture_all(&JS_IMANAORT, text, 1),
        Language::Go => capture_all(&GO_IMANAORT, text, 1),
    }
}

fn capture_all(re: &Regex, text: &str, group: usize) -> Vec<String> {
    re.captures_iter(text)
        .filter_map(|c| c.get(group).map(|m| m.as_str().to_string()))
        .collect()
}

fn dedup_preserving_order(items: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    items
        .into_iter()
        .filter(|i| seen.insert(i.clone()))
        .collect()
}

fn set_diff(a: &[String], b: &[String]) -> Vec<String> {
    let b_set: std::collections::HashSet<&str> = b.iter().map(String::as_str).collect();
    dedup_preserving_order(
        a.iter()
            .filter(|x| !b_set.contains(x.as_str()))
            .cloned()
            .collect(),
    )
}

/// Strip test-scoped regions from source so pattern checks only see production
/// code. v1 supports Rust: removes `#[cfg(test)] mod ... { ... }` and
/// `#[test] fn ... { ... }` blocks via brace-balanced line tracking. For any
/// other language the input is returned unchanged.
///
/// Limitations (acceptable for v1):
/// - Braces inside string literals / line comments are not skipped, so an
///   unbalanced `{` or `}` in a string can confuse depth tracking.
/// - Multi-line attribute stacks before a test item are handled, but raw
///   strings, doc comments, and macros that contain braces are not.
///
/// Returns the content with test blocks elided.
pub fn strip_test_blocks(file_path: &str, content: &str) -> String {
    let Some(Language::Rust) = language_of(file_path) else {
        return content.to_string();
    };
    strip_rust_test_blocks(content)
}

fn strip_rust_test_blocks(content: &str) -> String {
    let lines: Vec<&str> = content.lines().collect();
    let keep = rust_test_block_keep_mask(&lines);
    lines
        .iter()
        .zip(keep.iter())
        .filter(|(_, k)| **k)
        .map(|(line, _)| *line)
        .collect::<Vec<_>>()
        .join("\n")
}

/// For a `.rs` file's content, return a per-line mask where `true` means
/// "outside any `#[cfg(test)]` / `#[test]` block" and `false` means
/// "inside a test block." Same line-balancing rules as
/// `strip_rust_test_blocks`; exposed separately so callers (audit) can
/// skip test-block lines without losing line numbers.
pub fn rust_test_block_keep_mask_for(content: &str) -> Vec<bool> {
    let lines: Vec<&str> = content.lines().collect();
    rust_test_block_keep_mask(&lines)
}

fn rust_test_block_keep_mask(lines: &[&str]) -> Vec<bool> {
    let mut keep = vec![true; lines.len()];

    // Per-line state: whether a multi-line string remains open at end of line.
    // We pre-compute this so test-marker detection on each line can skip lines
    // that begin inside a string literal.
    let mut starts_in_str = vec![false; lines.len()];
    let mut cur_in_str = false;
    for (idx, line) in lines.iter().enumerate() {
        starts_in_str[idx] = cur_in_str;
        cur_in_str = update_in_str(line, cur_in_str);
    }

    let mut i = 0;
    while i < lines.len() {
        // Don't recognize test markers inside a multi-line string.
        if starts_in_str[i] {
            i += 1;
            continue;
        }
        let trimmed = lines[i].trim_start();
        let is_test_marker = trimmed.starts_with("#[cfg(test)]")
            || trimmed.starts_with("#[test]")
            || trimmed.starts_with("#[tokio::test]")
            || trimmed.starts_with("#[async_std::test]");
        if !is_test_marker {
            i += 1;
            continue;
        }

        let marker_idx = i;
        // Allow stacked attributes between the marker and the item.
        let mut j = i + 1;
        while j < lines.len() && !starts_in_str[j] && lines[j].trim_start().starts_with("#[") {
            j += 1;
        }
        if j >= lines.len() {
            break;
        }

        // Balance code-rank braces from j until depth returns to zero.
        let mut depth: i32 = 0;
        let mut block_started = false;
        let mut k = j;
        let mut in_str = starts_in_str[k];
        while k < lines.len() {
            let (op, cl) = count_code_braces(lines[k], &mut in_str);
            depth += op as i32;
            if op > 0 {
                block_started = true;
            }
            depth -= cl as i32;
            if block_started && depth <= 0 {
                for slot in &mut keep[marker_idx..=k] {
                    *slot = false;
                }
                i = k + 1;
                break;
            }
            k += 1;
        }
        if !block_started || depth > 0 {
            // Unbalanced or unterminated — bail out without further stripping.
            break;
        }
    }
    keep
}

/// Update the "inside string literal" flag after consuming `line`.
///
/// Tracks regular `"..."` strings (with `\` escapes). Strings opened on one
/// line and closed on a later line stay "in_str" between. Line comments end
/// the line so they can't span. Raw strings (`r"..."`, `r#"..."#`) and block
/// comments are NOT tracked in v1 — they are uncommon enough that we accept
/// the limitation.
fn update_in_str(line: &str, mut in_str: bool) -> bool {
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if in_str {
            if c == '\\' {
                chars.next();
                continue;
            }
            if c == '"' {
                in_str = false;
            }
        } else {
            if c == '/' && chars.peek() == Some(&'/') {
                return in_str; // line comment ends the line
            }
            if c == '"' {
                in_str = true;
            }
        }
    }
    in_str
}

/// Count `(open_braces, close_braces)` on `line`, ignoring characters inside
/// string literals and line comments. `in_str` is updated to reflect string
/// state at end of line so multi-line strings are tracked correctly.
fn count_code_braces(line: &str, in_str: &mut bool) -> (u32, u32) {
    let mut opens = 0u32;
    let mut closes = 0u32;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if *in_str {
            if c == '\\' {
                chars.next();
                continue;
            }
            if c == '"' {
                *in_str = false;
            }
            continue;
        }
        // Outside string
        if c == '/' && chars.peek() == Some(&'/') {
            break;
        }
        if c == '"' {
            *in_str = true;
            continue;
        }
        if c == '{' {
            opens += 1;
        } else if c == '}' {
            closes += 1;
        }
    }
    (opens, closes)
}

const CARGO_SUBCOMMANDS_NEEDING_WORKSPACE: &[&str] = &["build", "test", "check", "clippy"];

/// Cargo flags that scope the command to a subset of the workspace and
/// therefore make the `--workspace` recommendation moot. If any of these
/// flags appear in `content`, the rule suppresses all matches.
///
/// `-p` uses surrounding spaces (`" -p "`) to avoid false-matching against
/// hypothetical long flags like `--pretend`; `--package` is the long form
/// and is matched without surrounding spaces because long flags terminate
/// at `=` or whitespace.
const CARGO_SCOPING_FLAGS: &[&str] = &[
    "--workspace",
    " -p ",
    "--package",
    "--bin",
    "--bins",
    "--example",
    "--examples",
    "--lib",
    "--test",
    "--tests",
    "--bench",
    "--benches",
];

/// Scan `content` for cargo subcommands that benefit from `--workspace`
/// scoping. Returns the matched command excerpts (e.g. `"cargo build"`).
/// Returns empty if any cargo scoping flag (`--workspace`, `-p`, `--bin`,
/// etc.) appears in `content` — those flags indicate intentional scope.
///
/// Used on Bash content (no file extension), so this is a standalone fn
/// rather than a method on DiffFacts.
pub fn cargo_commands_lacking_workspace(content: &str) -> Vec<String> {
    if CARGO_SCOPING_FLAGS
        .iter()
        .any(|flag| content.contains(flag))
    {
        return Vec::new();
    }
    let mut out = Vec::new();
    for sub in CARGO_SUBCOMMANDS_NEEDING_WORKSPACE {
        let needle = format!("cargo {}", sub);
        if content.contains(&needle) {
            out.push(needle);
        }
    }
    out
}

// ─────────────────────────────────────────────────────────────────────
// Language regexes
//
// Regexes use `\b` and require non-alphanumeric prefix so we don't match
// e.g. `myfn name()` as a function-keyword match.
// ─────────────────────────────────────────────────────────────────────

// Rust: `fn name(`, `fn name<` (generics), pub/async/const/unsafe modifiers ok
static RUST_FN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?m)^\s*(?:pub(?:\([^)]*\))?\s+)?(?:(?:async|const|unsafe|extern(?:\s*"[^"]*")?)\s+)*fn\s+([A-Za-z_][A-Za-z0-9_]*)"#)
        .expect("RUST_FN regex compiles")
});
// `use foo::bar::Baz;` — capture the full path. We don't escoreand groups.
static RUST_USE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^\s*(?:pub\s+)?use\s+([A-Za-z_][A-Za-z0-9_:]*)").expect("RUST_USE compiles")
});

// Python: `def name(`, `async def name(`
static PY_FN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^\s*(?:async\s+)?def\s+([A-Za-z_][A-Za-z0-9_]*)\s*\(").expect("PY_FN compiles")
});
// `import foo` / `import foo.bar`
static PY_IMANAORT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^\s*import\s+([A-Za-z_][\w.]*)").expect("PY_IMANAORT compiles")
});
// `from foo.bar import ...`
static PY_FROM: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^\s*from\s+([A-Za-z_][\w.]*)\s+import").expect("PY_FROM compiles")
});

// JS/TS: `function name(`
static JS_FN_DECL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)(?:^|\s)(?:async\s+)?function\s+([A-Za-z_$][\w$]*)\s*\(")
        .expect("JS_FN_DECL compiles")
});
// `const name = (...) => ...` / `let name = function(...)`
static JS_FN_ASSIGN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?m)^\s*(?:escoreort\s+)?(?:const|let|var)\s+([A-Za-z_$][\w$]*)\s*=\s*(?:async\s+)?(?:\([^)]*\)\s*=>|function\b)"
    )
    .expect("JS_FN_ASSIGN compiles")
});
// `name(...) {` inside class bodies — heuristic, will produce some false positives
static JS_METHOD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?m)^\s{2,}(?:async\s+|static\s+|get\s+|set\s+)*([A-Za-z_$][\w$]*)\s*\([^)]*\)\s*\{",
    )
    .expect("JS_METHOD compiles")
});
// `import ... from 'module'` / `import 'module'`
static JS_IMANAORT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?m)^\s*import\s+(?:[^;]*\s+from\s+)?['"]([^'"]+)['"]"#)
        .expect("JS_IMANAORT compiles")
});

// Go: `func name(...)`, `func (recv T) name(...)`
static GO_FN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^\s*func\s+(?:\([^)]*\)\s+)?([A-Za-z_][A-Za-z0-9_]*)\s*\(")
        .expect("GO_FN compiles")
});
// `import "foo"` — only single-line form for v1 (no parenthesized block parsing)
static GO_IMANAORT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?m)^\s*import\s+(?:[A-Za-z_]\w*\s+)?"([^"]+)""#).expect("GO_IMANAORT compiles")
});

#[cfg(test)]
mod tests {
    use super::*;

    fn fns(file: &str, old: Option<&str>, new: &str) -> Vec<String> {
        extract(file, old, new).functions_added
    }

    fn removed(file: &str, old: &str, new: &str) -> Vec<String> {
        extract(file, Some(old), new).functions_removed
    }

    // ─── Rust ─────────────────────────────────────────────────────
    #[test]
    fn rust_extracts_added_functions() {
        let new = "pub fn foo() {} \n async fn bar() {} \n const fn baz() {}";
        assert_eq!(fns("file.rs", None, new), vec!["foo", "bar", "baz"]);
    }

    #[test]
    fn rust_extracts_function_removed() {
        let old = "fn keep() {} \n fn dropme() {}";
        let new = "fn keep() {}";
        assert_eq!(removed("file.rs", old, new), vec!["dropme"]);
    }

    #[test]
    fn rust_extracts_generic_function() {
        let new = "pub fn make<T: Clone>(x: T) -> T { x }";
        assert_eq!(fns("file.rs", None, new), vec!["make"]);
    }

    #[test]
    fn rust_extracts_unsafe_extern_fn() {
        let new = "unsafe fn raw() {}  \n  extern \"C\" fn cdecl() {}";
        let result = fns("file.rs", None, new);
        assert!(result.contains(&"raw".to_string()));
        assert!(result.contains(&"cdecl".to_string()));
    }

    #[test]
    fn rust_extracts_imports() {
        let new = "use std::collections::HashMap;\nuse serde::Serialize;";
        let facts = extract("file.rs", None, new);
        assert_eq!(
            facts.imports_added,
            vec!["std::collections::HashMap", "serde::Serialize"]
        );
    }

    #[test]
    fn rust_ignores_word_inside_identifier() {
        // "fn_name" is an identifier, not a `fn` keyword.
        let new = "let fn_name = 5;";
        assert!(fns("file.rs", None, new).is_empty());
    }

    // ─── Python ───────────────────────────────────────────────────
    #[test]
    fn python_extracts_def() {
        let new = "def hello():\n    pass\nasync def fetch():\n    pass";
        assert_eq!(fns("file.py", None, new), vec!["hello", "fetch"]);
    }

    #[test]
    fn python_extracts_imports() {
        let new = "import os\nimport requests\nfrom typing import List";
        let facts = extract("file.py", None, new);
        assert!(facts.imports_added.contains(&"os".to_string()));
        assert!(facts.imports_added.contains(&"requests".to_string()));
        assert!(facts.imports_added.contains(&"typing".to_string()));
    }

    // ─── JavaScript / TypeScript ──────────────────────────────────
    #[test]
    fn js_extracts_function_decl() {
        let new = "function greet() {}\nasync function load() {}";
        let result = fns("app.ts", None, new);
        assert!(result.contains(&"greet".to_string()));
        assert!(result.contains(&"load".to_string()));
    }

    #[test]
    fn js_extracts_arrow_function() {
        let new = "const add = (a, b) => a + b;\nescoreort const sub = async (a) => a;";
        let result = fns("app.js", None, new);
        assert!(result.contains(&"add".to_string()));
        assert!(result.contains(&"sub".to_string()));
    }

    #[test]
    fn js_extracts_import_from() {
        let new = "import React from 'react';\nimport { useState } from 'react';";
        let facts = extract("app.tsx", None, new);
        assert_eq!(facts.imports_added, vec!["react"]);
    }

    // ─── Go ───────────────────────────────────────────────────────
    #[test]
    fn go_extracts_function() {
        let new = "package main\nfunc main() {}\nfunc helper(x int) int { return x }";
        let result = fns("main.go", None, new);
        assert!(result.contains(&"main".to_string()));
        assert!(result.contains(&"helper".to_string()));
    }

    #[test]
    fn go_extracts_method() {
        let new = "func (s *Server) Start() error { return nil }";
        assert_eq!(fns("server.go", None, new), vec!["Start"]);
    }

    #[test]
    fn go_extracts_single_import() {
        let new = "import \"fmt\"\nimport mylog \"log\"";
        let facts = extract("main.go", None, new);
        assert_eq!(facts.imports_added, vec!["fmt", "log"]);
    }

    // ─── Cross-cutting ────────────────────────────────────────────
    #[test]
    fn unknown_extension_returns_empty() {
        assert_eq!(
            extract("file.unknown", None, "fn foo() {}"),
            DiffFacts::default()
        );
    }

    #[test]
    fn no_path_extension_returns_empty() {
        assert_eq!(
            extract("noext", None, "def foo(): pass"),
            DiffFacts::default()
        );
    }

    #[test]
    fn identical_old_new_yields_no_diff() {
        let code = "fn unchanged() {}";
        let facts = extract("file.rs", Some(code), code);
        assert!(facts.functions_added.is_empty());
        assert!(facts.functions_removed.is_empty());
    }

    #[test]
    fn write_mode_treats_everything_as_added() {
        let new = "fn a() {}\nfn b() {}";
        let facts = extract("file.rs", None, new);
        assert_eq!(facts.functions_added, vec!["a", "b"]);
        assert!(facts.functions_removed.is_empty());
    }

    #[test]
    fn duplicate_function_names_deduped() {
        // `extract` shouldn't list the same function twice if it appears in
        // both old and new at different states but with same name.
        let new = "fn foo() {}\n#[cfg(test)]\nmod tests { fn foo() {} }";
        let result = fns("file.rs", None, new);
        assert_eq!(result.iter().filter(|n| *n == "foo").count(), 1);
    }

    #[test]
    fn extraction_is_pathological_input_safe() {
        // Should complete quickly even on long, awkward input. (The `regex` crate
        // is non-backtracking, so this should be O(n).)
        let big = "fn x() {}\n".repeat(10_000);
        let start = std::time::Instant::now();
        let facts = extract("big.rs", None, &big);
        let elapsed = start.elapsed();
        assert!(
            elapsed.as_millis() < 500,
            "extraction took too long: {:?}",
            elapsed
        );
        assert_eq!(facts.functions_added, vec!["x"]);
    }

    #[test]
    fn rust_extract_diff_in_one_pass() {
        let old = "fn keep() {}\nfn drop_me() {}\nuse a::b;";
        let new = "fn keep() {}\nfn fresh() {}\nuse a::c;";
        let facts = extract("file.rs", Some(old), new);
        assert_eq!(facts.functions_added, vec!["fresh"]);
        assert_eq!(facts.functions_removed, vec!["drop_me"]);
        assert_eq!(facts.imports_added, vec!["a::c"]);
        assert_eq!(facts.imports_removed, vec!["a::b"]);
    }

    // ─── strip_test_blocks ────────────────────────────────────────
    #[test]
    fn strip_removes_cfg_test_mod() {
        let input = "\
fn keep() {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_works() {
        let x = thing.unwrap();
    }
}

fn also_keep() {}
";
        let out = strip_test_blocks("file.rs", input);
        assert!(out.contains("fn keep"));
        assert!(out.contains("fn also_keep"));
        assert!(!out.contains("mod tests"));
        assert!(!out.contains(".unwrap"));
    }

    #[test]
    fn strip_removes_inline_test_fn() {
        let input = "\
fn keep() {}

#[test]
fn test_foo() {
    assert_eq!(panic_handler.unwrap(), 1);
}

fn also_keep() {}
";
        let out = strip_test_blocks("file.rs", input);
        assert!(out.contains("fn keep"));
        assert!(out.contains("fn also_keep"));
        assert!(!out.contains("test_foo"));
        assert!(!out.contains(".unwrap"));
    }

    #[test]
    fn strip_handles_tokio_test_attr() {
        let input = "\
fn keep() {}

#[tokio::test]
async fn test_async() {
    let v = compute().await.unwrap();
}
";
        let out = strip_test_blocks("file.rs", input);
        assert!(!out.contains("test_async"));
        assert!(!out.contains(".unwrap"));
    }

    #[test]
    fn strip_handles_stacked_attributes() {
        let input = "\
fn keep() {}

#[cfg(test)]
#[allow(dead_code)]
mod tests {
    fn t() {
        x.unwrap();
    }
}
";
        let out = strip_test_blocks("file.rs", input);
        assert!(!out.contains("mod tests"));
        assert!(!out.contains(".unwrap"));
    }

    #[test]
    fn strip_preserves_non_test_content_with_nested_braces() {
        let input = "\
fn matrix() {
    let m = vec![vec![1, 2], vec![3, 4]];
    if true { do_stuff(); }
}

#[cfg(test)]
mod tests { fn t() {} }
";
        let out = strip_test_blocks("file.rs", input);
        assert!(out.contains("fn matrix"));
        assert!(out.contains("vec!"));
        assert!(out.contains("if true"));
        assert!(!out.contains("mod tests"));
    }

    #[test]
    fn strip_noop_for_non_rust() {
        let input = "#[cfg(test)]\nmod tests {\n.unwrap();\n}";
        // Python / other extensions: returned unchanged.
        assert_eq!(strip_test_blocks("file.py", input), input);
        assert_eq!(strip_test_blocks("file.ts", input), input);
        assert_eq!(strip_test_blocks("noext", input), input);
    }

    #[test]
    fn strip_handles_unbalanced_input_gracefully() {
        // Missing closing brace — should bail out without panic, leaving
        // remaining content visible. We don't fix what we can't parse.
        let input = "#[cfg(test)]\nmod tests {\n  fn t() {\n";
        let _ = strip_test_blocks("file.rs", input); // must not panic
    }

    #[test]
    fn strip_no_test_blocks_is_identity_modulo_trailing_newline() {
        let input = "fn keep() {}\nfn another() {}\n";
        // `.lines()` then `.join("\n")` drops the trailing newline; that's OK
        // for the use case (pattern checks).
        let out = strip_test_blocks("file.rs", input);
        assert!(out.contains("fn keep"));
        assert!(out.contains("fn another"));
    }

    // ─── cargo_commands_lacking_workspace ─────────────────────────
    #[test]
    fn cargo_build_alone_flags_workspace_omission() {
        assert_eq!(
            cargo_commands_lacking_workspace("cargo build"),
            vec!["cargo build".to_string()]
        );
    }

    #[test]
    fn cargo_build_with_workspace_does_not_flag() {
        assert!(cargo_commands_lacking_workspace("cargo build --workspace").is_empty());
    }

    #[test]
    fn cargo_test_with_p_alone_no_longer_flags() {
        // Was: asserted -p still flagged. Now: -p suppresses the warning
        // because it's intentional scoping (the agent is doing fast
        // inner-loop checks during iteration).
        assert!(cargo_commands_lacking_workspace("cargo test -p phr-mcp").is_empty());
    }

    #[test]
    fn cargo_build_with_p_flag_does_not_flag() {
        assert!(cargo_commands_lacking_workspace("cargo build -p phr-mcp").is_empty());
    }

    #[test]
    fn cargo_test_with_package_long_flag_does_not_flag() {
        assert!(cargo_commands_lacking_workspace("cargo test --package foo").is_empty());
    }

    #[test]
    fn cargo_check_with_bin_flag_does_not_flag() {
        assert!(cargo_commands_lacking_workspace("cargo check --bin server").is_empty());
    }

    #[test]
    fn cargo_build_with_example_flag_does_not_flag() {
        assert!(cargo_commands_lacking_workspace("cargo build --example demo").is_empty());
    }

    #[test]
    fn cargo_build_with_lib_flag_does_not_flag() {
        assert!(cargo_commands_lacking_workspace("cargo build --lib").is_empty());
    }

    #[test]
    fn cargo_test_with_test_flag_does_not_flag() {
        assert!(cargo_commands_lacking_workspace("cargo test --test integration").is_empty());
    }

    #[test]
    fn cargo_p_at_end_of_line_still_flags() {
        // Malformed: -p without value or trailing space. Suppression check
        // requires " -p " (surrounding spaces). Cargo would reject the command
        // anyway, so the warning here is acceptable noise on bad input.
        assert_eq!(
            cargo_commands_lacking_workspace("cargo build -p"),
            vec!["cargo build".to_string()]
        );
    }
}
