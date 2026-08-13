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
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

#[derive(Debug, Clone, Default)]
pub struct PackageIndex {
    by_import: BTreeMap<(String, String), BTreeSet<String>>,
    by_file: BTreeMap<String, String>,
}

fn package_name(content: &str) -> String {
    content
        .lines()
        .map(str::trim)
        .find_map(|line| {
            let rest = line.strip_prefix("package")?.trim_start();
            rest.split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                .next()
                .filter(|name| !name.is_empty())
                .map(str::to_string)
        })
        .unwrap_or_else(|| "_anonymous".to_string())
}

fn package_id(module: &str, file_path: &str, package: &str) -> String {
    let directory = file_path.rsplit_once('/').map_or("", |(dir, _)| dir);
    if directory.is_empty() {
        format!("cue:{module}::{package}")
    } else {
        format!("cue:{module}::{}::{package}", directory.replace('/', "::"))
    }
}

fn module_root(root: &Path, file_path: &str) -> (String, String) {
    let mut directory = root.join(file_path).parent().map(Path::to_path_buf);
    while let Some(current) = directory {
        let module_file = current.join("cue.mod/module.cue");
        if let Ok(content) = std::fs::read_to_string(module_file) {
            let relative = current
                .strip_prefix(root)
                .ok()
                .and_then(Path::to_str)
                .unwrap_or("")
                .replace('\\', "/");
            return (parse_module_name(&content), relative);
        }
        if current == root {
            break;
        }
        directory = current.parent().map(Path::to_path_buf);
    }
    ("project".to_string(), String::new())
}

pub fn build_package_index(root: &Path) -> PackageIndex {
    let mut index = PackageIndex::default();
    for file in discover_cue_files(root, "") {
        if file.ends_with("cue.mod/module.cue") {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(root.join(&file)) else {
            continue;
        };
        let package = package_name(&content);
        let (module, module_dir) = module_root(root, &file);
        let relative = file
            .strip_prefix(&format!("{module_dir}/"))
            .unwrap_or(&file);
        let id = package_id(&module, relative, &package);
        index.by_file.insert(file.clone(), id.clone());
        let directory = relative.rsplit_once('/').map_or("", |(dir, _)| dir);
        let import_path = if directory.is_empty() {
            module.clone()
        } else {
            format!("{module}/{directory}")
        };
        index
            .by_import
            .entry((import_path, package))
            .or_default()
            .insert(id);
    }
    index
}

fn import_literals(content: &str) -> Vec<String> {
    let mut imports = Vec::new();
    let mut in_block = false;
    for raw in content.lines() {
        let line = raw.split("//").next().unwrap_or("").trim();
        let import_body = line.strip_prefix("import").map(str::trim_start);
        if import_body.is_some_and(|body| body.starts_with('(')) {
            in_block = true;
            continue;
        }
        if in_block && line.starts_with(')') {
            in_block = false;
            continue;
        }
        if !(in_block || import_body.is_some()) {
            continue;
        }
        if let Some(start) = line.find('"')
            && let Some(end) = line[start + 1..].find('"')
        {
            imports.push(line[start + 1..start + 1 + end].to_string());
        }
    }
    imports
}

fn split_qualifier(path: &str) -> (&str, Option<&str>) {
    match path.rsplit_once(':') {
        Some((base, package)) if !package.contains('/') && !package.is_empty() => {
            (base, Some(package))
        }
        _ => (path, None),
    }
}

enum ImportResolution {
    Resolved(String),
    Builtin,
    Unresolved,
    Ambiguous,
}

fn resolve_indexed_import(path: &str, index: &PackageIndex) -> ImportResolution {
    const BUILTINS: &[&str] = &[
        "crypto", "encoding", "html", "list", "math", "net", "path", "regexp", "strconv",
        "strings", "struct", "text", "time", "tool",
    ];
    let (base, qualifier) = split_qualifier(path);
    if !base.contains('/') && BUILTINS.contains(&base) {
        return ImportResolution::Builtin;
    }
    let mut candidates = BTreeSet::new();
    for ((import_path, package), ids) in &index.by_import {
        if import_path == base && qualifier.is_none_or(|wanted| wanted == package) {
            candidates.extend(ids.iter().cloned());
        }
    }
    if candidates.len() == 1 {
        ImportResolution::Resolved(candidates.into_iter().next().expect("one candidate"))
    } else if candidates.is_empty() {
        ImportResolution::Unresolved
    } else {
        ImportResolution::Ambiguous
    }
}

fn code_shape(line: &str, in_block_comment: &mut bool) -> String {
    let mut out = String::with_capacity(line.len());
    let bytes = line.as_bytes();
    let mut i = 0;
    let mut quote = None;
    while i < bytes.len() {
        if *in_block_comment {
            if i + 1 < bytes.len() && bytes[i] == b'*' && bytes[i + 1] == b'/' {
                *in_block_comment = false;
                out.push_str("  ");
                i += 2;
            } else {
                out.push(' ');
                i += 1;
            }
        } else if let Some(q) = quote {
            if bytes[i] == b'\\' {
                out.push_str("  ");
                i += 2;
            } else {
                if bytes[i] == q {
                    quote = None;
                }
                out.push(' ');
                i += 1;
            }
        } else if i + 1 < bytes.len() && bytes[i] == b'/' && bytes[i + 1] == b'/' {
            out.extend(std::iter::repeat_n(' ', bytes.len() - i));
            break;
        } else if i + 1 < bytes.len() && bytes[i] == b'/' && bytes[i + 1] == b'*' {
            *in_block_comment = true;
            out.push_str("  ");
            i += 2;
        } else if bytes[i] == b'"' || bytes[i] == b'\'' {
            quote = Some(bytes[i]);
            out.push(' ');
            i += 1;
        } else {
            out.push(bytes[i] as char);
            i += 1;
        }
    }
    out
}

fn structural_definitions(content: &str) -> Vec<String> {
    let mut definitions = BTreeSet::new();
    let mut depth = 0usize;
    let mut scopes: Vec<(usize, String)> = Vec::new();
    let mut in_block_comment = false;
    for raw in content.lines() {
        let shaped = code_shape(raw, &mut in_block_comment);
        let trimmed = shaped.trim_start();
        let leading_closes = trimmed.chars().take_while(|c| *c == '}').count();
        depth = depth.saturating_sub(leading_closes);
        while scopes
            .last()
            .is_some_and(|(scope_depth, _)| *scope_depth > depth)
        {
            scopes.pop();
        }

        let label = trimmed
            .split_once(':')
            .map(|(candidate, _)| candidate.trim().trim_end_matches('?'))
            .filter(|candidate| {
                !candidate.is_empty()
                    && candidate
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '#')
            });
        let type_name = trimmed
            .strip_prefix("type ")
            .and_then(|rest| rest.split_once('=').map(|(name, _)| name.trim()))
            .filter(|name| {
                !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            });
        if depth == 0
            && let Some(name) = type_name
        {
            definitions.insert(name.to_string());
        }
        if let Some(label) = label {
            let is_definition = label.starts_with('#') || label.starts_with("_#");
            if is_definition || depth == 0 {
                let path = if is_definition {
                    scopes.last().map_or_else(
                        || label.to_string(),
                        |(_, parent)| format!("{parent}::{label}"),
                    )
                } else {
                    label.to_string()
                };
                definitions.insert(path.clone());
                if is_definition && trimmed.contains('{') {
                    scopes.push((depth + 1, path));
                }
            }
        }
        let opens = shaped.matches('{').count();
        let closes = shaped.matches('}').count().saturating_sub(leading_closes);
        depth = depth.saturating_add(opens).saturating_sub(closes);
        while scopes
            .last()
            .is_some_and(|(scope_depth, _)| *scope_depth > depth)
        {
            scopes.pop();
        }
    }
    definitions.into_iter().collect()
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
    extract_cue_with_index(file_path, content, unit, None)
}

pub fn extract_cue_at_root(root: &Path, file_path: &str, content: &str) -> Extracted {
    let index = build_package_index(root);
    let mut unit = UnitContext::unnamed_for(super::unit::LANG_CUE);
    unit.id = format!("cue:{}", discover_module(root, file_path));
    extract_cue_with_index(file_path, content, &unit, Some(&index))
}

pub(crate) fn extract_cue_with_index(
    file_path: &str,
    content: &str,
    unit: &UnitContext,
    index: Option<&PackageIndex>,
) -> Extracted {
    if !file_path.ends_with(".cue") {
        return Extracted::default();
    }

    // Empty / whitespace-only content contributes no edges and must not
    // compact away the file's prior evidence.
    if content.trim().is_empty() {
        return Extracted::unparseable();
    }

    let module = unit.id.strip_prefix("cue:").unwrap_or("project");
    let self_module = index
        .and_then(|packages| packages.by_file.get(file_path).cloned())
        .unwrap_or_else(|| package_id(module, file_path, &package_name(content)));
    let ft = file_type(file_path);
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

    for name in structural_definitions(content) {
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

    if let Some(packages) = index {
        for import_path in import_literals(content) {
            match resolve_indexed_import(&import_path, packages) {
                ImportResolution::Resolved(resolved) => {
                    out.insert(("imports".to_string(), vec![self_module.clone(), resolved]));
                }
                ImportResolution::Builtin => {}
                ImportResolution::Unresolved => {
                    out.insert((
                        "cue_import_diagnostic".to_string(),
                        vec![file_path.to_string(), import_path, "unresolved".to_string()],
                    ));
                }
                ImportResolution::Ambiguous => {
                    out.insert((
                        "cue_import_diagnostic".to_string(),
                        vec![file_path.to_string(), import_path, "ambiguous".to_string()],
                    ));
                }
            }
        }
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
    fn hidden_definitions_are_preserved() {
        let content = r#"
_#Internal: {}
"#;
        let out = extract_cue("schemas/schema.cue", content, &ctx());
        let defs = edges_of(&out, "defines");
        assert!(defs.iter().any(|a| a[1].ends_with("::_#Internal")));
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
    fn unresolved_imports_do_not_create_dangling_edges() {
        let content = r#"
import "example.com/acme/common"

common.Schema
"#;
        let out = extract_cue("schemas/schema.cue", content, &ctx());
        let imps = edges_of(&out, "imports");
        assert!(imps.is_empty());
    }

    #[test]
    fn indexed_unresolved_imports_emit_queryable_diagnostics() {
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(temp.path().join("cue.mod")).expect("module dir");
        std::fs::write(
            temp.path().join("cue.mod/module.cue"),
            "module: \"example.test\"\n",
        )
        .expect("module");
        let source = "package app\nimport \"example.test/missing\"\n";
        std::fs::write(temp.path().join("app.cue"), source).expect("app");
        let out = extract_cue_at_root(temp.path(), "app.cue", source);
        assert!(edges_of(&out, "imports").is_empty());
        assert_eq!(
            edges_of(&out, "cue_import_diagnostic"),
            vec![vec![
                "app.cue".to_string(),
                "example.test/missing".to_string(),
                "unresolved".to_string()
            ]]
        );
    }

    #[test]
    fn unqualified_ambiguous_imports_emit_diagnostics_not_edges() {
        let mut index = PackageIndex::default();
        index.by_import.insert(
            ("example.test/common".to_string(), "one".to_string()),
            BTreeSet::from(["cue:example.test::common::one".to_string()]),
        );
        index.by_import.insert(
            ("example.test/common".to_string(), "two".to_string()),
            BTreeSet::from(["cue:example.test::common::two".to_string()]),
        );
        let source = "package app\nimport \"example.test/common\"\n";
        let out = extract_cue_with_index("app.cue", source, &ctx(), Some(&index));
        assert!(edges_of(&out, "imports").is_empty());
        assert_eq!(edges_of(&out, "cue_import_diagnostic")[0][2], "ambiguous");
    }

    #[test]
    fn build_files_are_classified_as_build() {
        assert!(
            extract_cue("cue.mod/schema.cue", "package foo\n", &ctx())
                .edges
                .iter()
                .any(|e| e.p == "file_type" && e.a[1] == "build")
        );
        assert!(
            extract_cue("_tool.cue", "package tool\n", &ctx())
                .edges
                .iter()
                .any(|e| e.p == "file_type" && e.a[1] == "build")
        );
    }

    #[test]
    fn test_files_are_classified_as_test() {
        assert!(
            extract_cue("test/schema_test.cue", "package test\n", &ctx())
                .edges
                .iter()
                .any(|e| e.p == "file_type" && e.a[1] == "test")
        );
    }

    #[test]
    fn example_files_are_classified_as_example() {
        assert!(
            extract_cue("example/schema.cue", "package example\n", &ctx())
                .edges
                .iter()
                .any(|e| e.p == "file_type" && e.a[1] == "example")
        );
    }

    #[test]
    fn production_files_are_classified_as_production() {
        assert!(
            extract_cue("schemas/schema.cue", "package schemas\n", &ctx())
                .edges
                .iter()
                .any(|e| e.p == "file_type" && e.a[1] == "production")
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
        assert!(
            extract_cue("example/customer.cue", "package example\n", &ctx())
                .edges
                .iter()
                .any(|e| e.p == "file_type" && e.a[1] == "example")
        );
    }

    #[test]
    fn indexed_block_and_qualified_imports_resolve_packages() {
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(temp.path().join("cue.mod")).expect("module dir");
        std::fs::create_dir_all(temp.path().join("common")).expect("common dir");
        std::fs::create_dir_all(temp.path().join("app")).expect("app dir");
        std::fs::write(
            temp.path().join("cue.mod/module.cue"),
            "module: \"example.com/acme.game\"\n",
        )
        .expect("module");
        std::fs::write(
            temp.path().join("common/types.cue"),
            "package schema\n#Type: {}\n",
        )
        .expect("common");
        let source = r#"package app
import (
    alias "example.com/acme.game/common:schema"
    "list"
)
"#;
        std::fs::write(temp.path().join("app/main.cue"), source).expect("app");
        let out = extract_cue_at_root(temp.path(), "app/main.cue", source);
        let imps = edges_of(&out, "imports");
        assert_eq!(imps.len(), 1);
        assert_eq!(imps[0][1], "cue:example.com/acme.game::common::schema");
    }

    #[test]
    fn tabs_after_package_and_import_keywords_are_supported() {
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(temp.path().join("cue.mod")).expect("module dir");
        std::fs::create_dir_all(temp.path().join("common")).expect("common dir");
        std::fs::write(
            temp.path().join("cue.mod/module.cue"),
            "module: \"example.test\"\n",
        )
        .expect("module");
        std::fs::write(
            temp.path().join("common/value.cue"),
            "package\tcommon\n#Value: {}\n",
        )
        .expect("common");
        let source = "package\tapp\nimport\t\"example.test/common\"\n";
        std::fs::write(temp.path().join("app.cue"), source).expect("app");
        let out = extract_cue_at_root(temp.path(), "app.cue", source);
        assert_eq!(edges_of(&out, "imports").len(), 1);
        assert_eq!(
            edges_of(&out, "declares_module")[0][1],
            "cue:example.test::app"
        );
    }

    #[test]
    fn files_in_one_directory_and_package_share_a_node() {
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(temp.path().join("cue.mod")).expect("module dir");
        std::fs::create_dir_all(temp.path().join("schemas")).expect("schemas dir");
        std::fs::write(
            temp.path().join("cue.mod/module.cue"),
            "module: \"rulgamr.game\"\n",
        )
        .expect("module");
        for name in ["a.cue", "b.cue"] {
            std::fs::write(
                temp.path().join("schemas").join(name),
                "package manifest\n#Thing: {}\n",
            )
            .expect("source");
        }
        let a = extract_cue_at_root(
            temp.path(),
            "schemas/a.cue",
            "package manifest\n#Thing: {}\n",
        );
        let b = extract_cue_at_root(
            temp.path(),
            "schemas/b.cue",
            "package manifest\n#Thing: {}\n",
        );
        assert_eq!(
            edges_of(&a, "declares_module")[0][1],
            edges_of(&b, "declares_module")[0][1]
        );
        assert_eq!(
            edges_of(&a, "declares_module")[0][1],
            "cue:rulgamr.game::schemas::manifest"
        );
    }

    #[test]
    fn nested_definitions_survive_but_nested_data_fields_do_not() {
        let out = extract_cue(
            "schema.cue",
            "package p\n#Outer: {\n nested: string\n #Inner: { value: int }\n}\ntop: string\n// #Comment: {}\ntext: \"#String: {}\"\n",
            &ctx(),
        );
        let defs = edges_of(&out, "defines");
        assert!(defs.iter().any(|a| a[1].ends_with("::#Outer::#Inner")));
        assert!(defs.iter().any(|a| a[1].ends_with("::top")));
        assert!(!defs.iter().any(|a| a[1].ends_with("::nested")));
        assert!(!defs.iter().any(|a| a[1].contains("#Comment")));
        assert!(!defs.iter().any(|a| a[1].contains("#String")));
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
