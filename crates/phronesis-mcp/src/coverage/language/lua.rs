//! Lua coverage semantics: function sites with body ranges, the
//! production-source filter, and the brace-row one-liner rule (PLAN.md
//! Part K, decision 1 and 4). The walker trusts only node shapes the parse
//! probe confirmed against tree-sitter-lua 0.5.0: `function_declaration`
//! whose `name` field is an `identifier`, `dot_index_expression`
//! (`M.f`), or `method_index_expression` (`M:f`), and a `function_definition`
//! assigned in a `variable_declaration`/`assignment_statement` or held by a
//! table `field`. Anonymous callbacks are not sites.

use crate::coverage::region_map::{
    FunctionSite, RawSite, assign_function_ordinals, inclusive_end_line,
};

/// Function sites for one Lua source: declared functions (dotted and
/// method-style names included) and anonymous functions bound to a name
/// by assignment or a table field. Callbacks passed to a call are not
/// sites — their coverage belongs to the enclosing function.
pub fn lua_function_sites(source: &str) -> anyhow::Result<Vec<FunctionSite>> {
    let parsed = crate::syntax::parsed::ParsedFile::parse_lua(source)
        .ok_or_else(|| anyhow::anyhow!("tree-sitter failed to parse Lua source"))?;
    let crate::syntax::parsed::ParsedFile::Lua { tree, source: text } = &parsed else {
        anyhow::bail!("parse_lua returned a non-Lua tree");
    };
    let mut raw = Vec::new();
    walk(tree.root_node(), text.as_bytes(), &mut raw);
    Ok(assign_function_ordinals(raw))
}

fn walk(node: tree_sitter::Node<'_>, src: &[u8], out: &mut Vec<RawSite>) {
    match node.kind() {
        "function_declaration" => {
            // `function M.f()`, `function M:f()`, `local function f()` —
            // the `name` field carries the qualification by itself
            // (`M.f` -> M::f, `M:f` -> M::f, dotted chains -> a::b::c).
            if let Some(name) = node.child_by_field_name("name")
                && let Some(path) = qualified_name(name, src)
            {
                record(node, &path, out);
            }
        }
        "function_definition" => {
            // An anonymous `function() end` is a site only when a name
            // binds it: the target of an assignment (`M.k = function()`,
            // `local one = function()`) or a table field's value
            // (`{ run = function() end }`). Anything else — a call
            // argument, a returned literal — is anonymous and belongs to
            // its enclosing function.
            if let Some(path) = assigned_name(node, src) {
                record(node, &path, out);
            }
        }
        _ => {}
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk(child, src, out);
    }
}

/// The `::`-joined item path of a declaration or assignment target:
/// `f`, `M.f` -> `M::f`, `M:f` -> `M::f`, `a.b.c` -> `a::b::c`
/// (decision 1: the method colon collapses into the same item path).
fn qualified_name(node: tree_sitter::Node<'_>, src: &[u8]) -> Option<String> {
    match node.kind() {
        "identifier" => node.utf8_text(src).ok().map(str::to_string),
        "dot_index_expression" | "method_index_expression" => {
            let table = node.child_by_field_name("table")?;
            let field = node
                .child_by_field_name("field")
                .or_else(|| node.child_by_field_name("method"))?;
            let mut path = qualified_name(table, src)?;
            path.push_str("::");
            path.push_str(field.utf8_text(src).ok()?);
            Some(path)
        }
        _ => None,
    }
}

/// The binding name of an assigned `function_definition`: the assignment
/// target (`variable_list`'s `name`) for assignments, or the table
/// constructor's field name prefixed by the assigned variable (`local t =
/// { run = function() end }` -> `t::run`).
fn assigned_name(node: tree_sitter::Node<'_>, src: &[u8]) -> Option<String> {
    let parent = node.parent()?;
    match parent.kind() {
        "field" => {
            // field -> table_constructor -> expression_list -> assignment
            // -> variable_list name; the variable names the table.
            let key = parent.child_by_field_name("name")?;
            let key = key.utf8_text(src).ok()?;
            let table_var = table_variable(parent, src)?;
            Some(format!("{table_var}::{key}"))
        }
        "expression_list" => {
            let assignment = parent.parent()?;
            if assignment.kind() != "assignment_statement" {
                return None;
            }
            let target = assignment
                .children(&mut assignment.walk())
                .find(|c| c.kind() == "variable_list")?;
            let name = target.child_by_field_name("name")?;
            qualified_name(name, src)
        }
        _ => None,
    }
}

/// The variable a table constructor is assigned to: the
/// `variable_list` target of the `assignment_statement` holding the
/// `table_constructor` that holds this `field`.
fn table_variable(field: tree_sitter::Node<'_>, src: &[u8]) -> Option<String> {
    let mut node = field.parent()?; // table_constructor
    while let Some(parent) = node.parent() {
        if parent.kind() == "assignment_statement" {
            let target = parent
                .children(&mut parent.walk())
                .find(|c| c.kind() == "variable_list")?;
            return qualified_name(target.child_by_field_name("name")?, src);
        }
        node = parent;
    }
    None
}

fn record(node: tree_sitter::Node<'_>, path: &str, out: &mut Vec<RawSite>) {
    let start_line = node.start_position().row as u64 + 1;
    let body_start_line = node
        .child_by_field_name("body")
        .and_then(|body| {
            body.named_children(&mut body.walk())
                .find(|child| !child.is_extra() && child.kind() != "comment")
                .map(|statement| statement.start_position().row as u64 + 1)
        })
        .unwrap_or(start_line);
    out.push(RawSite {
        path: path.to_string(),
        start_line,
        body_start_line,
        end_line: inclusive_end_line(node),
    });
}

/// Decision 4: production sources are `.lua` files the graph's own
/// classifier calls production — not spec/test trees, not `*_spec.lua` —
/// and not vendored dependency trees. `.rockspec` build files never match
/// the registry's extension, so they cannot arrive here.
pub fn is_wanted_source(rel: &str) -> bool {
    if !rel.ends_with(".lua") {
        return false;
    }
    if crate::graph::lua::file_type(rel) != "production" {
        return false;
    }
    !rel.split('/')
        .any(|seg| seg == ".luarocks" || seg == "lua_modules")
}

/// Revision 2 brace-row rule: a one-liner is a true single-line site
/// (`local one = function() return 1 end`); multi-line Lua functions end
/// on a later line, and luacov's lcov has no `FN`/`FNDA`, so with
/// `one_liner_needs_fnda: true` a one-line Lua function is always
/// reported `unattributable` and never guessed from the declaration line.
/// The same ambiguity applies when the first body statement shares its
/// header line, even if the function ends on a later line.
pub fn is_one_liner(site: &FunctionSite) -> bool {
    site.end_line == site.start_line || site.body_start_line <= site.start_line
}

/// The runnable command `coverage select` renders: the busted filter form,
/// with the runner-native full name derived from the graph id (`busted
/// --filter '<full name>' <spec>`; PLAN.md Task K3).
pub fn render_command(test_id: &str, file: &str) -> Option<String> {
    let name = runner_name(test_id, file)?;
    Some(format!("busted --filter '{name}' {file}"))
}

/// The runner-native name for a busted test: the suffix after the spec
/// file's module marker — describe titles and the it title, converted to
/// the space-joined form busted builds its filter names from
/// (`busted/modules/filter_loader.lua`). Extracted the way
/// `python::render_command` extracts its pytest node id, never a naive
/// last-segment split: titles may contain spaces, and a title may even
/// contain `::` (such a title renders the wrong filter, and the
/// collection guard catches that loudly rather than silently).
pub fn runner_name(test_id: &str, file: &str) -> Option<String> {
    let module = file.strip_suffix(".lua")?.replace('/', "::");
    let marker = format!("::{module}::");
    let (_, title) = test_id.rsplit_once(&marker)?;
    Some(title.replace("::", " "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lua_sites_cover_dotted_methods_local_assigned_table_and_one_liners() {
        let src = "\
local M = {}

function M.f()
  return 1
end

function M:g()
  return 2
end

local function h()
  return 3
end

function outer.inner()
  return 4
end

M.k = function()
  return 5
end

local one = function() return 1 end

local t = { run = function() return 7 end }

arr.map(function(x) return x end)
";
        let sites = lua_function_sites(src).expect("parses");
        let rows: Vec<(&str, u64, u64, u64)> = sites
            .iter()
            .map(|s| {
                (
                    s.item_path.as_str(),
                    s.start_line,
                    s.body_start_line,
                    s.end_line,
                )
            })
            .collect();
        assert_eq!(
            rows,
            vec![
                ("M::f", 3, 4, 5),
                ("M::g", 7, 8, 9),
                ("h", 11, 12, 13),
                ("outer::inner", 15, 16, 17),
                ("M::k", 19, 20, 21),
                ("one", 23, 23, 23),
                ("t::run", 25, 25, 25),
            ],
            "{sites:?}"
        );
        // The anonymous `arr.map(function(x) ...)` callback is not a site.
        assert!(!sites.iter().any(|s| s.item_path.contains("arr")));
        assert_eq!(sites.len(), 7);
    }

    #[test]
    fn lua_sources_follow_decision_4() {
        assert!(is_wanted_source("src/store.lua"));
        assert!(is_wanted_source("lua/lib/util.lua"));
        assert!(!is_wanted_source("spec/store_spec.lua"));
        assert!(!is_wanted_source("src/store_spec.lua"));
        assert!(!is_wanted_source("spec/helper.lua"));
        assert!(!is_wanted_source("tests/store_test.lua"));
        assert!(!is_wanted_source(".luarocks/share/lua/5.4/x.lua"));
        assert!(!is_wanted_source("lua_modules/x/init.lua"));
        assert!(!is_wanted_source("store.rockspec"));
    }

    #[test]
    fn lua_one_liner_rule_is_the_brace_row_form() {
        let one_line = FunctionSite {
            item_path: "one".into(),
            start_line: 4,
            body_start_line: 4,
            end_line: 4,
        };
        let multi_line = FunctionSite {
            item_path: "g".into(),
            start_line: 4,
            body_start_line: 4,
            end_line: 6,
        };
        assert!(is_one_liner(&one_line));
        assert!(
            is_one_liner(&multi_line),
            "header-sharing bodies are ambiguous"
        );
    }

    #[test]
    fn runner_name_is_the_busted_full_name_after_the_module_marker() {
        // busted composes full names by joining describe titles and the it
        // title with spaces (busted/modules/filter_loader.lua), so the
        // `::`-joined id suffix converts to the runner-native name.
        assert_eq!(
            runner_name(
                "lua:myapp::spec::store_spec::store::loads the store",
                "spec/store_spec.lua"
            )
            .as_deref(),
            Some("store loads the store")
        );
        assert_eq!(
            runner_name("lua:myapp::spec::store_spec::plain", "spec/store_spec.lua").as_deref(),
            Some("plain")
        );
        assert_eq!(
            runner_name("lua:myapp::spec::store_spec::store::loads", "src/store.lua"),
            None,
            "an id whose marker does not match the spec file names nothing"
        );
    }

    /// A pinning regression (the honesty contract of plan K decision 2):
    /// luacov's lcov reporter emits no FN/FNDA, so through the real lua
    /// registry row a one-line function is unattributable even when its own
    /// declaration line carries a positive DA — never guessed.
    #[test]
    fn one_line_lua_sites_are_unattributable_without_fnda() {
        use crate::coverage::lcov::{LcovSource, hit_sites};
        let lua = crate::coverage::language::language_for_path("src/store.lua").expect("lua row");
        let sites =
            lua_function_sites("function M.f() return 1 end\n\nfunction M.g()\n  return 2\nend\n")
                .expect("sites");
        // luacov -r lcov shape: DA lines only, no FN/FNDA.
        let src = LcovSource {
            path: "src/store.lua".into(),
            function_hits: vec![],
            line_hits: vec![(1, 1), (4, 1)],
        };
        let (hit, unattr) = hit_sites(&sites, &src, lua);
        assert_eq!(hit.len(), 1, "multi-line M::g hits from its body line");
        assert_eq!(unattr.len(), 1, "the one-liner is reported unattributable");
        assert_eq!(unattr[0].item_path, "M::f");
    }

    #[test]
    fn lua_body_hits_ignore_declaration_and_comment_lines() {
        use crate::coverage::lcov::{LcovSource, hit_sites};
        let lua = crate::coverage::language::language_for_path("src/store.lua").expect("lua");
        let sites = lua_function_sites("function M.load()\n  -- explanation\n  return 1\nend\n")
            .expect("sites");
        assert_eq!(sites[0].body_start_line, 3);
        let mut src = LcovSource {
            path: "src/store.lua".into(),
            function_hits: vec![],
            line_hits: vec![(1, 1), (2, 1), (3, 0)],
        };
        let (hit, unattr) = hit_sites(&sites, &src, lua);
        assert!(hit.is_empty(), "loading the declaration is not a call");
        assert!(unattr.is_empty(), "separate body line can be attributed");
        src.line_hits[2].1 = 1;
        assert_eq!(hit_sites(&sites, &src, lua).0.len(), 1);
    }

    #[test]
    fn lua_header_sharing_and_empty_bodies_require_function_evidence() {
        use crate::coverage::lcov::{LcovSource, hit_sites};
        let lua = crate::coverage::language::language_for_path("src/store.lua").expect("lua");
        let sites = lua_function_sites(
            "function M.load() local value = 1\n  return value\nend\nfunction M.empty()\nend\n",
        )
        .expect("sites");
        assert_eq!(sites.len(), 2);
        let mut src = LcovSource {
            path: "src/store.lua".into(),
            function_hits: vec![],
            line_hits: vec![(1, 1), (2, 0), (4, 1), (5, 1)],
        };
        let (hit, unattr) = hit_sites(&sites, &src, lua);
        assert!(hit.is_empty());
        assert_eq!(unattr.len(), 2);
        src.function_hits.push(("load".into(), 1));
        assert_eq!(hit_sites(&sites, &src, lua).0.len(), 1);
    }

    #[test]
    fn render_command_pins_the_busted_filter_form() {
        assert_eq!(
            render_command(
                "lua:myapp::spec::store_spec::store::loads the stored value",
                "spec/store_spec.lua"
            )
            .as_deref(),
            Some("busted --filter 'store loads the stored value' spec/store_spec.lua")
        );
        assert_eq!(
            render_command("lua:myapp::spec::store_spec::store::loads", "src/store.lua"),
            None,
            "an id whose marker does not match the spec file renders nothing"
        );
    }

    #[test]
    fn registry_dispatches_lua_paths_and_test_ids() {
        use crate::coverage::language::{language_for_path, language_for_test_id};
        let lua = language_for_path("src/store.lua").expect("lua row");
        assert_eq!(lua.id, "lua");
        assert_eq!(lua.test_id_prefix, "lua:");
        assert!(lua.one_liner_needs_fnda);
        assert!((lua.is_wanted_source)("src/store.lua"));
        assert!(!(lua.is_wanted_source)("spec/store_spec.lua"));
        assert_eq!(
            language_for_test_id("lua:myapp::spec::store_spec::store::loads").map(|l| l.id),
            Some("lua")
        );
    }
}
