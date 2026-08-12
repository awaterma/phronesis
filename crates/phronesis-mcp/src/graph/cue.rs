//! The CUE sensor: derives structural edges from one CUE source file.
//!
//! Uses regex extraction — CUE has no maintained tree-sitter grammar in the
//! workspace's version, and the language's constraint model makes AST-level
//! claims inherently ambiguous (spec §6 non-goal).
//!
//! Constraint definitions (#Name), type definitions (type Foo =), and field
//! definitions (Name: or Name =) all map to graph_definition + defines +
//! element_in_file + element_in_module, not defines_fn.

use super::extract::Extracted;
use super::model::Edge;
use super::unit::UnitContext;
use std::collections::BTreeSet;
use std::path::Path;
use std::sync::LazyLock;

/// Regex matching CUE constraint definitions: `#Name` followed by `:` and
/// then `{` (optionally with content and/or closing brace on the same line).
static CONSTRAINT_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?m)^[\t ]*#(\w+):.*\{").expect("static regex compiles"));

/// Regex matching CUE type definitions: `type <Name> = ...`.
static TYPE_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"(?m)^[\t ]*type\s+(\w+)\s*=").expect("static regex compiles")
});

/// Regex matching CUE field/definition lines: a bare top-level identifier
/// followed by `:`, `:=`, or `=`.
static FIELD_DEF_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"(?m)^[\t ]*([A-Za-z]\w*)(?::=|:\s|\s*=)").expect("static regex compiles")
});

/// Regex matching standard imports: `import "..."` or `import '...'`.
static IMPORT_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r#"import\s+(?:local\s+)?["']([^"']+)["']"#).expect("static regex compiles")
});

/// Resolve a CUE import path to a language-qualified module path.
///
/// Handles:
/// - Relative paths starting with `.` — resolved against the file's directory
/// - Absolute module paths — converted to `cue:<module_path>` form
fn resolve_import(import_path: &str, file_path: &str, cue_files: &[String]) -> String {
    // Relative import: resolve against the file's directory.
    if import_path.starts_with('.') {
        let dir = file_path.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
        let mut path = dir.to_string();

        for component in import_path.split('/') {
            match component {
                ".." => {
                    if let Some(pos) = path.rfind('/') {
                        path.truncate(pos);
                    } else {
                        path.clear();
                    }
                }
                "" | "." => {}
                seg => {
                    if !path.is_empty() {
                        path.push('/');
                    }
                    path.push_str(seg);
                }
            }
        }

        // Check exact match in cue files.
        for candidate in [&path, &format!("{path}/init.cue")] {
            if cue_files.iter().any(|f| f.as_str() == candidate) {
                return cue_module_path(candidate);
            }
        }

        // Fallback: construct path from the resolved directory.
        let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        if segments.is_empty() {
            return "cue:.".to_string();
        }
        return format!("cue:{}", segments.join("::"));
    }

    // Standard import: convert path to `cue:<module_path>` form.
    let normalized = import_path.replace('.', "/");
    let stripped = normalized.strip_suffix(".cue").unwrap_or(&normalized);
    let segments: Vec<&str> = stripped.split('/').filter(|s| !s.is_empty()).collect();
    if segments.is_empty() {
        return "cue:.".to_string();
    }
    format!("cue:{}", segments.join("::"))
}

/// Classification of a `.cue` file as test, build, example, or production.
///
/// Follows the spec's precedence: test and example are directory-based, build
/// is cue.mod/ and _tool files, everything else is production.
fn file_type(file_path: &str) -> &'static str {
    // Test files: *_test.cue, test/, tests/
    if file_path.ends_with("_test.cue")
        || file_path.starts_with("test/")
        || file_path.starts_with("tests/")
        || file_path.contains("/test/")
        || file_path.contains("/tests/")
    {
        return "test";
    }

    // Build files: cue.mod/*.cue, _*.cue, package tool
    if file_path.starts_with("cue.mod/")
        || file_path.ends_with("_tool.cue")
        || file_path.ends_with("_gen.cue")
    {
        return "build";
    }

    // Example files: example/
    if file_path.starts_with("example/") || file_path.contains("/example/") {
        return "example";
    }

    "production"
}

/// Build the language-qualified module path for a `.cue` file.
///
/// `schemas/workload/deployment.cue` →
/// `cue:my-module::schemas::workload::deployment`
fn cue_module_path(file_path: &str) -> String {
    let trimmed = file_path.strip_suffix(".cue").unwrap_or(file_path);
    let segments: Vec<&str> = trimmed.split('/').filter(|s| !s.is_empty()).collect();
    format!("cue:{}", segments.join("::"))
}

/// Discover the module path from a project's `cue.mod/module.cue`.
///
/// Parses the file's `module` declaration to extract the module path. Falls
/// back to `project` if no module is declared or the file is missing.
pub fn discover_module(root: &Path, file_path: &str) -> String {
    let mut directory = root.join(file_path).parent().map(Path::to_path_buf);
    while let Some(current) = directory {
        let module_file = current.join("cue.mod/module.cue");
        if let Ok(content) = std::fs::read_to_string(module_file) {
            return parse_module_name(&content);
        }
        if current == root {
            break;
        }
        directory = current.parent().map(Path::to_path_buf);
    }
    "project".to_string()
}

/// Extract the module name from a `cue.mod/module.cue` file.
///
/// Parses the `module` key's literal string value. Does not evaluate
/// `@if` constraints or non-literal values (spec §6 non-goal).
fn parse_module_name(content: &str) -> String {
    // Match `module "example.com/path"` or `module: "example.com/path"`
    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(name) = trimmed
            .strip_prefix("module")
            .and_then(|s| s.strip_prefix(':'))
        {
            let name = name.trim();
            if let Some(start) = name.find('"')
                && let Some(end) = name[start + 1..].find('"')
            {
                let mod_name = &name[start + 1..start + 1 + end];
                if !mod_name.is_empty() {
                    return mod_name.to_string();
                }
            }
        } else if let Some(name) = trimmed.strip_prefix("module") {
            let name = name.trim();
            if let Some(start) = name.find('"')
                && let Some(end) = name[start + 1..].find('"')
            {
                let mod_name = &name[start + 1..start + 1 + end];
                if !mod_name.is_empty() {
                    return mod_name.to_string();
                }
            }
        }
    }
    "project".to_string()
}

/// Discover every tracked `.cue` file under `root`.
///
/// Walks the repository using the same ignore-based traversal as
/// `super::sync::tracked_files`, returning repo-relative paths sorted
/// deterministically.
pub fn discover_cue_files(root: &Path, _file_path: &str) -> Vec<String> {
    let mut out = Vec::new();
    for entry in ignore::WalkBuilder::new(root)
        .hidden(true)
        .filter_entry(|e| e.file_name() != "node_modules")
        .build()
        .flatten()
    {
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let path = entry.path();
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            if ext != "cue" {
                continue;
            }
        } else {
            continue;
        }
        if let Ok(rel) = path.strip_prefix(root)
            && let Some(rel) = rel.to_str()
        {
            out.push(rel.replace('\\', "/"));
        }
    }
    out.sort();
    out
}

/// Extract every base relation from one CUE file.
pub fn extract_cue(file_path: &str, content: &str, unit: &UnitContext) -> Extracted {
    if !file_path.ends_with(".cue") {
        return Extracted::default();
    }

    // Empty / whitespace-only content contributes no edges and must not
    // compact away the file's prior evidence.
    if content.trim().is_empty() {
        return Extracted::unparseable();
    }

    let path = file_path.strip_suffix(".cue").unwrap_or(file_path);
    let self_module = format!("{}::{}", unit.id, path.replace('/', "::"));
    let ft = file_type(file_path);
    let cue_files = &unit.cue_files;

    let mut out: BTreeSet<(String, Vec<String>)> = BTreeSet::new();

    // Always emit file_type and declares_module.
    out.insert((
        "file_type".to_string(),
        vec![file_path.to_string(), ft.to_string()],
    ));
    out.insert((
        "declares_module".to_string(),
        vec![file_path.to_string(), self_module.clone()],
    ));

    // Extract constraint definitions: #Name: { ... }
    for cap in CONSTRAINT_RE.captures_iter(content) {
        let name = cap.get(1).map_or("", |m| m.as_str());
        if !name.is_empty() {
            let qualified = format!("{self_module}::#{name}");
            out.insert(("graph_definition".to_string(), vec![qualified.clone()]));
            out.insert((
                "defines".to_string(),
                vec![file_path.to_string(), qualified.clone()],
            ));
            out.insert((
                "element_in_file".to_string(),
                vec![qualified.clone(), file_path.to_string()],
            ));
            out.insert((
                "element_in_module".to_string(),
                vec![qualified, self_module.clone()],
            ));
        }
    }

    // Extract type definitions: type Foo = Bar
    for cap in TYPE_RE.captures_iter(content) {
        let name = cap.get(1).map_or("", |m| m.as_str());
        if !name.is_empty() {
            let qualified = format!("{self_module}::{name}");
            out.insert(("graph_definition".to_string(), vec![qualified.clone()]));
            out.insert((
                "defines".to_string(),
                vec![file_path.to_string(), qualified.clone()],
            ));
            out.insert((
                "element_in_file".to_string(),
                vec![qualified.clone(), file_path.to_string()],
            ));
            out.insert((
                "element_in_module".to_string(),
                vec![qualified, self_module.clone()],
            ));
        }
    }

    // Extract field/definition lines: Name: ... or Name = ...
    for cap in FIELD_DEF_RE.captures_iter(content) {
        let name = cap.get(1).map_or("", |m| m.as_str());
        if !name.is_empty() {
            // Skip type keyword matches (already handled above).
            if name == "type" {
                continue;
            }
            // Skip constraint defs that were already captured.
            if name.starts_with('#') {
                continue;
            }
            let qualified = format!("{self_module}::{name}");
            out.insert(("graph_definition".to_string(), vec![qualified.clone()]));
            out.insert((
                "defines".to_string(),
                vec![file_path.to_string(), qualified.clone()],
            ));
            out.insert((
                "element_in_file".to_string(),
                vec![qualified.clone(), file_path.to_string()],
            ));
            out.insert((
                "element_in_module".to_string(),
                vec![qualified, self_module.clone()],
            ));
        }
    }

    // Extract import statements and resolve to imports edges.
    for cap in IMPORT_RE.captures_iter(content) {
        let import_path = cap.get(1).map_or("", |m| m.as_str());
        let resolved = resolve_import(import_path, file_path, cue_files);
        out.insert(("imports".to_string(), vec![self_module.clone(), resolved]));
    }

    // Risk watchlist: calls to APIs that pose security concerns.
    let watchlist = ["cuecmd", "cueimports"];
    for api in watchlist {
        let pattern = format!(r"(?m)\b{}\s*\(", api);
        if let Ok(re) = regex::Regex::new(&pattern)
            && re.is_match(content)
        {
            out.insert((
                "calls_api".to_string(),
                vec![self_module.clone(), api.to_string()],
            ));
        }
    }

    let edges = out
        .into_iter()
        .map(|(p, a)| Edge {
            p,
            a,
            src: file_path.to_string(),
            d: false,
        })
        .collect();

    Extracted {
        edges,
        skipped: 0,
        parse_failed: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> UnitContext {
        UnitContext {
            id: "cue:my-module".to_string(),
            module_base: String::new(),
            siblings: std::collections::BTreeMap::new(),
            ts: super::super::unit::TsConfig::default(),
            files: Vec::new(),
            lua_files: Vec::new(),
            cue_files: Vec::new(),
        }
    }

    fn edges_of(x: &Extracted, p: &str) -> Vec<Vec<String>> {
        x.edges
            .iter()
            .filter(|e| e.p == p)
            .map(|e| e.a.clone())
            .collect()
    }

    #[test]
    fn non_cue_files_return_empty() {
        let out = extract_cue("foo.py", "pass\n", &ctx());
        assert!(out.edges.is_empty());
    }

    #[test]
    fn empty_content_returns_unparseable() {
        let out = extract_cue("foo.cue", "", &ctx());
        assert!(out.parse_failed);
        assert!(out.edges.is_empty());
    }

    #[test]
    fn whitespace_only_content_returns_unparseable() {
        let out = extract_cue("foo.cue", "   \n  \n  ", &ctx());
        assert!(out.parse_failed);
    }

    #[test]
    fn a_constraint_becomes_graph_definition() {
        let content = r#"
#IsPositive: {
    _ > 0
}
"#;
        let out = extract_cue("schemas/schema.cue", content, &ctx());
        let defs = edges_of(&out, "defines");
        assert!(defs.iter().any(|a| a[1].ends_with("::#IsPositive")));
    }

    #[test]
    fn hidden_definitions_are_skipped() {
        let content = r#"
_#Internal: {}
"#;
        let out = extract_cue("schemas/schema.cue", content, &ctx());
        let defs = edges_of(&out, "defines");
        assert!(defs.iter().all(|a| a[1] != "_#Internal"));
    }

    #[test]
    fn a_type_definition_becomes_graph_definition() {
        let content = r#"
type Deployment = {
    apiVersion: string
    kind: string
}
"#;
        let out = extract_cue("schemas/deployment.cue", content, &ctx());
        let defs = edges_of(&out, "defines");
        assert!(defs.iter().any(|a| a[1].ends_with("::Deployment")));
    }

    #[test]
    fn imports_become_imports_edges() {
        let content = r#"
import "example.com/acme/common"

common.Schema
"#;
        let out = extract_cue("schemas/schema.cue", content, &ctx());
        let imps = edges_of(&out, "imports");
        assert_eq!(imps.len(), 1);
        // resolve_import converts dots to slashes, so example.com -> example::com
        assert!(imps[0][1].starts_with("cue:example::com::acme::common"));
    }

    #[test]
    fn build_files_are_classified_as_build() {
        assert_eq!(
            extract_cue("cue.mod/schema.cue", "package foo\n", &ctx())
                .edges
                .iter()
                .any(|e| e.p == "file_type" && e.a[1] == "build"),
            true
        );
        assert_eq!(
            extract_cue("_tool.cue", "package tool\n", &ctx())
                .edges
                .iter()
                .any(|e| e.p == "file_type" && e.a[1] == "build"),
            true
        );
    }

    #[test]
    fn test_files_are_classified_as_test() {
        assert_eq!(
            extract_cue("test/schema_test.cue", "package test\n", &ctx())
                .edges
                .iter()
                .any(|e| e.p == "file_type" && e.a[1] == "test"),
            true
        );
    }

    #[test]
    fn example_files_are_classified_as_example() {
        assert_eq!(
            extract_cue("example/schema.cue", "package example\n", &ctx())
                .edges
                .iter()
                .any(|e| e.p == "file_type" && e.a[1] == "example"),
            true
        );
    }

    #[test]
    fn production_files_are_classified_as_production() {
        assert_eq!(
            extract_cue("schemas/schema.cue", "package schemas\n", &ctx())
                .edges
                .iter()
                .any(|e| e.p == "file_type" && e.a[1] == "production"),
            true
        );
    }

    #[test]
    fn declares_module_edge_maps_file_to_module() {
        let out = extract_cue("schemas/deployment.cue", "package schemas\n", &ctx());
        let decls = edges_of(&out, "declares_module");
        assert_eq!(decls.len(), 1);
        assert_eq!(decls[0][0], "schemas/deployment.cue");
        assert!(decls[0][1].starts_with("cue:"));
    }

    #[test]
    fn file_type_classifies_example_files() {
        assert_eq!(
            extract_cue("example/customer.cue", "package example\n", &ctx())
                .edges
                .iter()
                .any(|e| e.p == "file_type" && e.a[1] == "example"),
            true
        );
    }

    #[test]
    fn import_resolution_resolves_module_path() {
        let files = vec![
            "schemas/schema.cue".to_string(),
            "example/common.cue".to_string(),
        ];
        let unit = UnitContext {
            id: "cue:my-module".to_string(),
            module_base: String::new(),
            siblings: std::collections::BTreeMap::new(),
            ts: super::super::unit::TsConfig::default(),
            files: Vec::new(),
            lua_files: Vec::new(),
            cue_files: files,
        };
        let out = extract_cue(
            "schemas/schema.cue",
            r#"import "example.com/acme/common"

common.Schema
"#,
            &unit,
        );
        let imps = edges_of(&out, "imports");
        assert_eq!(imps.len(), 1);
        assert!(imps[0][1].starts_with("cue:"));
    }

    #[test]
    fn simple_field_becomes_graph_definition() {
        let content = r#"
Name: string
Age: int
"#;
        let out = extract_cue("schemas/schema.cue", content, &ctx());
        let defs = edges_of(&out, "defines");
        assert!(defs.iter().any(|a| a[1].ends_with("::Name")));
        assert!(defs.iter().any(|a| a[1].ends_with("::Age")));
    }

    #[test]
    fn multiple_definitions_in_one_file() {
        let content = r#"
#IsPositive: {}
type Number = int
Count: int
Name: string
"#;
        let out = extract_cue("schemas/all.cue", content, &ctx());
        let defs = edges_of(&out, "defines");
        assert!(defs.iter().any(|a| a[1].ends_with("::#IsPositive")));
        assert!(defs.iter().any(|a| a[1].ends_with("::Number")));
        assert!(defs.iter().any(|a| a[1].ends_with("::Count")));
        assert!(defs.iter().any(|a| a[1].ends_with("::Name")));
    }
}
