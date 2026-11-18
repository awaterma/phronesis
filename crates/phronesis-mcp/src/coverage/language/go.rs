//! Go coverage uses the same validated pack declarations as the code graph.
use crate::coverage::{language::CoverageLanguage, region_map::FunctionSite};
#[cfg(feature = "go-grammar")]
use crate::language_packs::{self, Capability};
#[cfg(feature = "go-grammar")]
use anyhow::Context;
use anyhow::{Result, bail};
use std::path::Path;

pub const LANGUAGE: CoverageLanguage = CoverageLanguage {
    id: "go",
    extensions: &["go"],
    test_id_prefix: "go:",
    function_sites: without_root,
    function_sites_for_path: None,
    is_wanted_source: |path| !path.ends_with("_test.go"),
    is_one_liner: |_| false,
    one_liner_needs_fnda: false,
    // Go commands come from pack_test_command, never from guessed ID parsing.
    render_command: |_, _| None,
    render_command_with_tool: None,
};

fn without_root(_source: &str) -> Result<Vec<FunctionSite>> {
    bail!("Go coverage requires repository context; use extract_function_sites_in")
}

/// Encode the pack's percent escapes into the coverage identifier alphabet.
/// Escape underscores first so literal names cannot collide with encoded bytes.
pub fn region_item(symbol: &str) -> String {
    symbol.replace('_', "_u").replace('%', "_p")
}

#[cfg(feature = "go-grammar")]
pub fn function_sites(root: &Path, file: &str, source: &str) -> Result<Vec<FunctionSite>> {
    if source.is_empty() {
        return Ok(Vec::new());
    }
    let registry = language_packs::load(root)?;
    let pack = registry
        .for_path_with(file, Capability::Graph)?
        .context("Go graph pack is not available")?;
    let context = language_packs::file_context(root, file, pack)?;
    let extraction = language_packs::extract(pack, &context, source)?;
    if !extraction.diagnostics.is_empty() {
        bail!("Go coverage extraction reported adapter diagnostics");
    }
    let edges = language_packs::graph_records(pack, &context, &extraction)?;
    let mut parser = tree_sitter::Parser::new();
    parser.set_language(&tree_sitter_go::LANGUAGE.into())?;
    let tree = parser
        .parse(source, None)
        .context("Go coverage parse failed")?;
    if tree.root_node().has_error() {
        bail!("Go coverage source contains syntax errors");
    }
    let mut sites = Vec::new();
    for edge in edges
        .iter()
        .filter(|e| e.p == "pack_declaration" && e.a.len() >= 8 && e.a[5] == "false")
    {
        let start: usize = edge.a[6].parse()?;
        let end: usize = edge.a[7].parse()?;
        let span = source
            .get(start..end)
            .context("invalid Go declaration span")?;
        if span.is_empty() {
            continue;
        }
        let node = tree
            .root_node()
            .descendant_for_byte_range(start, end.saturating_sub(1))
            .context("missing Go declaration node")?;
        let Some(body) = node.child_by_field_name("body") else {
            continue;
        };
        // A top-level initializer on the body-opening row can supply the
        // positive DA count even when this function never ran. Discard that
        // row rather than assigning its count to the function.
        let body_row = body.start_position().row;
        let mut cursor = tree.root_node().walk();
        let ambiguous_start = tree.root_node().named_children(&mut cursor).any(|other| {
            other.id() != node.id()
                && !matches!(
                    other.kind(),
                    "comment" | "package_clause" | "import_declaration"
                )
                && other.start_position().row <= body_row
                && other.end_position().row >= body_row
        });
        let mut body_start_line = body_row as u64 + 1 + u64::from(ambiguous_start);
        // Advancing past an ambiguous row must not turn a multiline body
        // into a one-line-body attribution candidate on its closing row.
        if body_row != body.end_position().row
            && body_start_line == body.end_position().row as u64 + 1
        {
            body_start_line += 1;
        }
        sites.push(FunctionSite {
            item_path: region_item(&edge.a[1]),
            start_line: source[..start].bytes().filter(|b| *b == b'\n').count() as u64 + 1,
            body_start_line,
            end_line: source[..end]
                .trim_end_matches('\n')
                .bytes()
                .filter(|b| *b == b'\n')
                .count() as u64
                + 1,
        });
    }
    Ok(sites)
}

#[cfg(not(feature = "go-grammar"))]
pub fn function_sites(_root: &Path, _file: &str, _source: &str) -> Result<Vec<FunctionSite>> {
    bail!("Go coverage requires the go-grammar feature")
}
