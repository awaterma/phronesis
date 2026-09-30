use crate::coverage::region_map::FunctionSite;
use anyhow::{Context, Result};
use std::collections::BTreeMap;
use tree_sitter::Node;

#[derive(Clone)]
struct RawSite {
    path: String,
    start: u64,
    body: u64,
    end: u64,
}

pub fn java_function_sites(source: &str) -> Result<Vec<FunctionSite>> {
    let parsed = crate::syntax::parsed::ParsedFile::parse_java(source)
        .context("tree-sitter failed to parse Java source")?;
    let crate::syntax::parsed::ParsedFile::Java { tree, source } = &parsed else {
        anyhow::bail!("parse_java returned a non-Java tree");
    };
    fn walk(node: Node<'_>, bytes: &[u8], scope: &mut Vec<String>, raw: &mut Vec<RawSite>) {
        let pushed = if matches!(
            node.kind(),
            "class_declaration"
                | "interface_declaration"
                | "enum_declaration"
                | "record_declaration"
        ) {
            node.child_by_field_name("name")
                .and_then(|n| n.utf8_text(bytes).ok())
                .map(|s| {
                    scope.push(s.to_owned());
                })
        } else {
            None
        };
        let name = match node.kind() {
            "method_declaration" if node.child_by_field_name("body").is_some() => {
                node.child_by_field_name("name")
            }
            "constructor_declaration" | "compact_constructor_declaration" => {
                node.child_by_field_name("name")
            }
            _ => None,
        };
        if let Some(name) = name.and_then(|n| n.utf8_text(bytes).ok()) {
            let method = if matches!(
                node.kind(),
                "constructor_declaration" | "compact_constructor_declaration"
            ) {
                "<init>"
            } else {
                name
            };
            let mut parts = scope.clone();
            parts.push(method.into());
            let body = node.child_by_field_name("body").or_else(|| {
                let mut c = node.walk();
                node.children(&mut c)
                    .find(|c| matches!(c.kind(), "block" | "constructor_body"))
            });
            if let Some(body) = body {
                raw.push(RawSite {
                    path: parts.join("::"),
                    start: node.start_position().row as u64 + 1,
                    body: body.start_position().row as u64 + 1,
                    end: node.end_position().row as u64 + 1,
                });
            }
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            walk(child, bytes, scope, raw);
        }
        if pushed.is_some() {
            scope.pop();
        }
    }
    let mut raw = Vec::new();
    walk(
        tree.root_node(),
        source.as_bytes(),
        &mut Vec::new(),
        &mut raw,
    );
    let mut seen = BTreeMap::<String, u32>::new();
    Ok(raw
        .into_iter()
        .map(|r| {
            let ordinal = seen
                .entry(r.path.clone())
                .and_modify(|n| *n += 1)
                .or_insert(1);
            FunctionSite {
                item_path: if *ordinal > 1 {
                    format!("{}.{ordinal}", r.path)
                } else {
                    r.path
                },
                start_line: r.start,
                body_start_line: r.body,
                end_line: r.end,
            }
        })
        .collect())
}

pub fn is_wanted_source(rel: &str) -> bool {
    rel.ends_with(".java")
        && rel.contains("/src/main/java/")
        && !rel.contains("/target/")
        && !rel.contains("/build/")
        && ![
            "Test.java",
            "Tests.java",
            "IT.java",
            "module-info.java",
            "package-info.java",
        ]
        .iter()
        .any(|suffix| rel.ends_with(suffix))
}

pub fn is_one_liner(site: &FunctionSite) -> bool {
    site.end_line == site.start_line
}

pub fn render_command(_test_id: &str, _file: &str) -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn java_sites_cover_methods_constructors_nested_classes_overloads_and_skip_abstract() {
        let src = "package com.x;\npublic class Store {\n public Store() {\n }\n public int load() {\n return 1;\n }\n public int load(int again) {\n return again;\n }\n public int oneLiner() { return 3; }\n static class Inner {\n int deep() {\n return 2;\n }\n }\n interface Api { int missing(); }\n}\n";
        let sites = java_function_sites(src).expect("parses");
        let rows: Vec<_> = sites
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
                ("Store::<init>", 3, 3, 4),
                ("Store::load", 5, 5, 7),
                ("Store::load.2", 8, 8, 10),
                ("Store::oneLiner", 11, 11, 11),
                ("Store::Inner::deep", 13, 13, 15)
            ]
        );
    }
}
