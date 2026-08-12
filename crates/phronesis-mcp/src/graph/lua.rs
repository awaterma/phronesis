//! The Lua sensor: derives structural edges from one Lua source file.
//!
//! Uses regex extraction — the task explicitly accepts this for Lua, where
//! tree-sitter grammar coverage for the subset we need (function defs,
//! require calls) is not maintained in the workspace's tree-sitter version,
//! and the Lua language's dynamic loader model makes AST-level claims
//! inherently ambiguous (spec §6 non-goal).

use super::extract::Extracted;
use super::model::Edge;
use super::unit::UnitContext;
use std::collections::BTreeSet;
use std::sync::LazyLock;

/// Regex matching Lua function definitions:
/// - optional `local` keyword
/// - `function` keyword
/// - optional `prefix.` for `M.foo` / `self.bar` style
/// - the function name
/// - opening paren (allows `:` method style via `:?`)
///
/// Groups: [0] full match, [1] optional table prefix (e.g. `M`), [2] function name.
static FUNC_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"(?m)^(\s*(?:local\s+)?)?function\s+(?:(\w+)\.)?(\w+)\s*(?::?)\s*\(")
        .expect("static regex compiles")
});

/// Classify a `.lua` file as `test`, `build`, or `production`.
fn file_type(file_path: &str) -> &'static str {
    if file_path.starts_with("test/")
        || file_path.starts_with("tests/")
        || file_path.starts_with("spec/")
        || file_path.contains("/test/")
        || file_path.contains("/tests/")
        || file_path.contains("/spec/")
        || file_path.ends_with("_test.lua")
        || file_path.ends_with("_spec.lua")
    {
        return "test";
    }
    if file_path.ends_with(".rockspec") || file_path.ends_with("build.lua") {
        return "build";
    }
    "production"
}

/// Build the language-qualified module path for a `.lua` file.
///
/// `src/utils/helpers.lua` with unit ID `lua:myproject` →
/// `lua:myproject::src::utils::helpers`
fn module_path(file_path: &str, unit: &UnitContext) -> String {
    let trimmed = file_path.strip_suffix(".lua").unwrap_or(file_path);
    let segments: Vec<&str> = trimmed.split('/').filter(|s| !s.is_empty()).collect();
    std::iter::once(unit.id.as_str())
        .chain(segments)
        .collect::<Vec<_>>()
        .join("::")
}

/// Resolve a literal `require` argument to a language-qualified module path.
///
/// Handles:
/// - Relative paths starting with `.` — resolved against the file's directory
/// - Unit-relative `@pkg.mod` — resolved against the unit's file list
/// - Standard `a.b.c` — resolved by looking up known files
fn resolve_require(
    require_arg: &str,
    file_path: &str,
    unit: &UnitContext,
    files: &[String],
) -> String {
    // Relative require: resolve against the file's directory.
    if require_arg.starts_with('.') {
        let dir = file_path.rsplit_once('/').map(|(d, _)| d).unwrap_or("");

        // Walk `..` segments.
        let mut path = dir.to_string();
        for component in require_arg.split('/') {
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

        // Check exact match or init.lua variant.
        for candidate in [&path, &format!("{path}/init.lua")] {
            if let Some(file) = files.iter().find(|f| f.as_str() == candidate) {
                return module_path(file, unit);
            }
        }

        // Fallback: construct path from the resolved directory.
        let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        return std::iter::once(unit.id.as_str())
            .chain(segments)
            .collect::<Vec<_>>()
            .join("::");
    }

    // Unit-relative: `@myproject.utils` → resolve within this unit.
    if let Some(rest) = require_arg.strip_prefix('@') {
        let dot_separated = rest.replace('.', "/");
        let candidate = format!("{rest}.lua");
        let slash_separated = format!("{dot_separated}.lua");

        if let Some(file) = files
            .iter()
            .find(|f| f.as_str() == candidate || f.as_str() == slash_separated)
        {
            return module_path(file, unit);
        }
        // Fallback to unit ID + module segments.
        let segments: Vec<&str> = rest.split('/').filter(|s| !s.is_empty()).collect();
        return std::iter::once(unit.id.as_str())
            .chain(segments)
            .collect::<Vec<_>>()
            .join("::");
    }

    // Standard module: look up in known files.
    let dot_separated = require_arg.replace('.', "/");
    let candidates = [
        format!("{require_arg}.lua"),
        format!("{dot_separated}.lua"),
        format!("{require_arg}/init.lua"),
        format!("{dot_separated}/init.lua"),
    ];

    for candidate in &candidates {
        if let Some(file) = files.iter().find(|f| f.as_str() == candidate) {
            return module_path(file, unit);
        }
    }

    // Fallback: use the raw require arg as module path segments.
    let segments: Vec<&str> = require_arg.split('/').filter(|s| !s.is_empty()).collect();
    std::iter::once(unit.id.as_str())
        .chain(segments)
        .collect::<Vec<_>>()
        .join("::")
}

/// Extract every base relation from one Lua file.
pub fn extract_lua(file_path: &str, content: &str, unit: &UnitContext) -> Extracted {
    if !file_path.ends_with(".lua") {
        return Extracted::default();
    }

    // Empty / whitespace-only content contributes no edges and must not
    // compact away the file's prior evidence.
    if content.trim().is_empty() {
        return Extracted::unparseable();
    }

    let self_module = module_path(file_path, unit);
    let ft = file_type(file_path);
    let files = &unit.lua_files;

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

    // Extract function definitions.
    for cap in FUNC_RE.captures_iter(content) {
        // cap[2] is optional table prefix (M, self, etc.), cap[3] is func name.
        let prefix = cap.get(2).map_or("", |m| m.as_str());
        let func_name = cap.get(3).map_or("", |m| m.as_str());
        if func_name.is_empty() {
            continue;
        }

        let fn_name = if prefix.is_empty() {
            func_name.to_string()
        } else {
            format!("{prefix}.{func_name}")
        };

        let qualified = format!("{self_module}.{fn_name}");

        out.insert((
            "defines_fn".to_string(),
            vec![file_path.to_string(), qualified],
        ));
    }

    // Extract require() calls and resolve to imports edges.
    let require_re = regex::Regex::new(r#"require\s*\(\s*['"]([^'"]+)['"]\s*\)"#).unwrap();
    for cap in require_re.captures_iter(content) {
        let arg = cap.get(1).map_or("", |m| m.as_str());
        let resolved = resolve_require(arg, file_path, unit, files);
        out.insert(("imports".to_string(), vec![self_module.clone(), resolved]));
    }

    // Risk watchlist: calls to APIs that pose security concerns.
    // Split into dynamic-code-load (high-signal) and general calls_api.
    let dynamic_loaders = ["dofile", "load", "loadfile", "loadstring"];
    let general_watchlist = ["assert", "setfenv", "module"];
    for api in &dynamic_loaders {
        let pattern = format!(r"(?m)\b{}\s*\(", api);
        if let Ok(re) = regex::Regex::new(&pattern)
            && re.is_match(content)
        {
            out.insert((
                "lua_dynamic_code_load".to_string(),
                vec![file_path.to_string(), self_module.clone(), api.to_string()],
            ));
        }
    }
    for api in &general_watchlist {
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

    fn ctx(id: &str) -> UnitContext {
        UnitContext {
            id: id.to_string(),
            module_base: String::new(),
            siblings: std::collections::BTreeMap::new(),
            ts: crate::graph::unit::TsConfig::default(),
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
    fn non_lua_files_return_empty() {
        let out = extract_lua("foo.py", "pass\n", &ctx("lua:test"));
        assert!(out.edges.is_empty());
    }

    #[test]
    fn empty_content_returns_unparseable() {
        let out = extract_lua("foo.lua", "", &ctx("lua:test"));
        assert!(out.parse_failed);
        assert!(out.edges.is_empty());
    }

    #[test]
    fn whitespace_only_content_returns_unparseable() {
        let out = extract_lua("foo.lua", "   \n  \n  ", &ctx("lua:test"));
        assert!(out.parse_failed);
    }

    #[test]
    fn a_simple_function_becomes_defines_fn() {
        let out = extract_lua(
            "src/main.lua",
            "function hello()\n  print('hi')\nend\n",
            &ctx("lua:myapp"),
        );
        let defs = edges_of(&out, "defines_fn");
        assert_eq!(defs.len(), 1);
        assert!(defs[0][1].contains("hello"));
    }

    #[test]
    fn local_function_becomes_defines_fn() {
        let out = extract_lua(
            "src/main.lua",
            "local function foo() return 1 end\n",
            &ctx("lua:myapp"),
        );
        let defs = edges_of(&out, "defines_fn");
        assert_eq!(defs.len(), 1);
        assert!(defs[0][1].contains("foo"));
    }

    #[test]
    fn table_method_becomes_defines_fn() {
        let out = extract_lua(
            "src/utils/helpers.lua",
            "function M.greet(name)\n  return 'hello ' .. name\nend\n",
            &ctx("lua:myapp"),
        );
        let defs = edges_of(&out, "defines_fn");
        assert_eq!(defs.len(), 1);
        assert!(defs[0][1].contains("M.greet"));
    }

    #[test]
    fn file_type_classifies_test_files() {
        assert_eq!(
            extract_lua("tests/foo.lua", "return {}\n", &ctx("lua:test"))
                .edges
                .iter()
                .any(|e| e.p == "file_type" && e.a[1] == "test"),
            true
        );
        assert_eq!(
            extract_lua("src/foo_spec.lua", "return {}\n", &ctx("lua:test"))
                .edges
                .iter()
                .any(|e| e.p == "file_type" && e.a[1] == "test"),
            true
        );
    }

    #[test]
    fn file_type_classifies_build_files() {
        assert_eq!(
            extract_lua("build.lua", "return {}\n", &ctx("lua:test"))
                .edges
                .iter()
                .any(|e| e.p == "file_type" && e.a[1] == "build"),
            true
        );
    }

    #[test]
    fn file_type_classifies_production() {
        assert_eq!(
            extract_lua("src/main.lua", "return {}\n", &ctx("lua:test"))
                .edges
                .iter()
                .any(|e| e.p == "file_type" && e.a[1] == "production"),
            true
        );
    }

    #[test]
    fn declares_module_edge_maps_file_to_module() {
        let out = extract_lua("src/utils/helpers.lua", "return {}\n", &ctx("lua:myapp"));
        let decls = edges_of(&out, "declares_module");
        assert_eq!(decls.len(), 1);
        assert_eq!(decls[0][0], "src/utils/helpers.lua");
        assert_eq!(decls[0][1], "lua:myapp::src::utils::helpers");
    }

    #[test]
    fn require_becomes_imports_edge() {
        let out = extract_lua(
            "src/main.lua",
            "local m = require('core')\n",
            &ctx("lua:myapp"),
        );
        let imps = edges_of(&out, "imports");
        assert_eq!(imps.len(), 1);
        // The resolved path starts with the unit id.
        assert!(imps[0][1].starts_with("lua:myapp::"));
    }

    #[test]
    fn dynamic_loaders_emit_lua_dynamic_code_load() {
        let content = "dofile('script.lua')\nloadfile('other.lua')()\nloadstring('x = 1')()\n";
        let out = extract_lua("src/main.lua", content, &ctx("lua:myapp"));
        let dyn_loads = edges_of(&out, "lua_dynamic_code_load");
        assert_eq!(dyn_loads.len(), 3);
        assert!(dyn_loads.iter().any(|a| a[2] == "dofile"));
        assert!(dyn_loads.iter().any(|a| a[2] == "loadfile"));
        assert!(dyn_loads.iter().any(|a| a[2] == "loadstring"));
    }

    #[test]
    fn calls_api_does_not_detect_safe_code() {
        let out = extract_lua("src/main.lua", "local x = 1\nprint(x)\n", &ctx("lua:myapp"));
        let apis = edges_of(&out, "calls_api");
        assert!(apis.is_empty());
    }

    #[test]
    fn multiple_functions_in_one_file() {
        let content = r#"
function a() return 1 end
local function b() return 2 end
function M.c() return 3 end
"#;
        let out = extract_lua("src/all.lua", content, &ctx("lua:myapp"));
        let defs = edges_of(&out, "defines_fn");
        assert_eq!(defs.len(), 3);
    }

    #[test]
    fn require_resolves_relative_paths() {
        let files = vec!["src/main.lua".to_string(), "src/core/init.lua".to_string()];
        let unit = UnitContext {
            id: "lua:myapp".to_string(),
            module_base: String::new(),
            siblings: std::collections::BTreeMap::new(),
            ts: crate::graph::unit::TsConfig::default(),
            files: Vec::new(),
            lua_files: files.clone(),
            cue_files: Vec::new(),
        };
        let out = extract_lua("src/main.lua", "local m = require('./core')\n", &unit);
        let imps = edges_of(&out, "imports");
        assert_eq!(imps.len(), 1);
        assert!(imps[0][1].contains("core"));
    }

    #[test]
    fn general_watchlist_emits_calls_api() {
        let out = extract_lua(
            "src/main.lua",
            "assert(true, 'fail')\nsetfenv(1, {})\nmodule('m')\n",
            &ctx("lua:myapp"),
        );
        let apis = edges_of(&out, "calls_api");
        assert!(apis.iter().any(|a| a[1] == "assert"));
        assert!(apis.iter().any(|a| a[1] == "setfenv"));
        assert!(apis.iter().any(|a| a[1] == "module"));
    }
}
