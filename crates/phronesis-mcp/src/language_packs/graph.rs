//! Generic graph translation for typed pack records.
use super::{Extraction, FileContext, PackError, PackRecord, ReferenceMode, ValidatedPack};
use crate::graph::model::Edge;
use std::collections::{BTreeMap, BTreeSet};

fn escape(part: &str) -> String {
    let mut out = String::new();
    for byte in part.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b'/') {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn scope_id(pack: &ValidatedPack, scope: &str) -> String {
    format!(
        "{}:{}::{}",
        pack.assets.manifest.language,
        pack.assets.manifest.id,
        escape(scope)
    )
}

fn site(file: &str, start: usize) -> String {
    format!("{}@{}", escape(file), start)
}

fn symbol(scope: &str, name: &str, receiver: Option<&str>) -> String {
    match receiver {
        Some(receiver) => format!("{scope}::{}::{}", escape(receiver), escape(name)),
        None => format!("{scope}::{}", escape(name)),
    }
}

/// Translate validated records into source-attributed graph edges. Reference
/// resolution is deliberately a separate whole-graph operation.
pub fn graph_records(
    pack: &ValidatedPack,
    context: &FileContext,
    extraction: &Extraction,
) -> Result<Vec<Edge>, PackError> {
    let file = &context.file_rel;
    let id = &pack.assets.manifest.id;
    let has_test = extraction
        .records
        .iter()
        .any(|record| matches!(record, PackRecord::Declaration { test: true, .. }));
    let mut edges = vec![
        Edge::base(
            "pack_file_adapter",
            &[file, id, &pack.assets.manifest.version, &pack.digest],
            file,
        ),
        Edge::base(
            "file_type",
            &[file, if has_test { "test" } else { "production" }],
            file,
        ),
    ];
    let mut scopes = BTreeSet::new();
    for record in &extraction.records {
        match record {
            PackRecord::Declaration {
                name,
                scope,
                receiver,
                span,
                test,
                command,
            } => {
                let scope = scope_id(pack, scope);
                scopes.insert(scope.clone());
                let declaration_site = site(file, span.start_byte);
                let function = symbol(&scope, name, receiver.as_deref());
                let start = span.start_byte.to_string();
                let end = span.end_byte.to_string();
                edges.push(Edge::base(
                    "pack_declaration",
                    &[
                        &declaration_site,
                        &function,
                        &scope,
                        name,
                        receiver.as_deref().unwrap_or(""),
                        if *test { "true" } else { "false" },
                        &start,
                        &end,
                    ],
                    file,
                ));
                edges.push(Edge::base("defines_fn", &[file, &function], file));
                if receiver.is_some() {
                    edges.push(Edge::base("defines_method", &[file, &function], file));
                }
                if *test {
                    edges.push(Edge::base("defines_test", &[file, &function], file));
                    if let Some(command) = command {
                        let argv = serde_json::to_string(&command.args).map_err(|e| {
                            PackError::Record {
                                pack: Some(id.clone()),
                                path: Some(file.clone()),
                                message: e.to_string(),
                            }
                        })?;
                        edges.push(Edge::base(
                            "pack_test_command",
                            &[&function, &command.cwd, &command.program, &argv],
                            file,
                        ));
                    }
                }
            }
            PackRecord::Reference {
                owner_start_byte,
                target,
                scope,
                resolution,
                span,
            } => {
                let scope = scope_id(pack, scope);
                scopes.insert(scope.clone());
                let owner_site = site(file, *owner_start_byte);
                let start = span.start_byte.to_string();
                let end = span.end_byte.to_string();
                let mode = if *resolution == ReferenceMode::SameScope {
                    "same_scope"
                } else {
                    "unresolved"
                };
                edges.push(Edge::base(
                    "pack_reference",
                    &[&owner_site, &scope, target, mode, &start, &end],
                    file,
                ));
            }
            PackRecord::Import {
                scope,
                target,
                alias,
                span,
            } => {
                let scope = scope_id(pack, scope);
                scopes.insert(scope.clone());
                edges.push(Edge::base(
                    "pack_import",
                    &[&scope, target, alias.as_deref().unwrap_or("")],
                    file,
                ));
                let unresolved = format!("pack-unresolved:{}:{}", id, escape(target));
                edges.push(Edge::base("imports", &[&scope, &unresolved], file));
                let _ = span;
            }
            PackRecord::Fact { .. } => {}
        }
    }
    for scope in scopes {
        edges.push(Edge::base("declares_module", &[file, &scope], file));
    }
    Ok(edges)
}

/// Resolve only unique free declarations in the exact same pack scope.
/// Count distinct declaration sites before symbol deduplication. A query may
/// emit one identical declaration per parameter of a Go function; those are
/// one site. Distinct build-tag definitions remain ambiguous.
pub fn resolve_pack_references(base: &[Edge]) -> Vec<Edge> {
    let mut definitions: BTreeMap<(String, String), BTreeMap<String, String>> = BTreeMap::new();
    let mut owners: BTreeMap<String, (String, String, bool)> = BTreeMap::new();
    for edge in base.iter().filter(|e| !e.d && e.p == "pack_declaration") {
        if edge.a.len() < 8 {
            continue;
        }
        owners.insert(
            edge.a[0].clone(),
            (edge.a[1].clone(), edge.a[2].clone(), edge.a[5] == "true"),
        );
        if edge.a[4].is_empty() {
            definitions
                .entry((edge.a[2].clone(), edge.a[3].clone()))
                .or_default()
                .insert(edge.a[0].clone(), edge.a[1].clone());
        }
    }
    let mut out = BTreeSet::new();
    for edge in base.iter().filter(|e| !e.d && e.p == "pack_reference") {
        if edge.a.len() < 6 || edge.a[3] != "same_scope" {
            continue;
        }
        let Some((owner, owner_scope, is_test)) = owners.get(&edge.a[0]) else {
            continue;
        };
        if owner_scope != &edge.a[1] {
            continue;
        }
        let Some(candidates) = definitions.get(&(edge.a[1].clone(), edge.a[2].clone())) else {
            continue;
        };
        if candidates.len() != 1 {
            continue;
        }
        let Some(target) = candidates.values().next() else {
            continue;
        };
        if *is_test {
            out.insert(EdgeKey(
                "tested_by",
                target.clone(),
                owner.clone(),
                edge.src.clone(),
            ));
        } else {
            out.insert(EdgeKey(
                "calls",
                owner.clone(),
                target.clone(),
                edge.src.clone(),
            ));
        }
    }
    out.into_iter()
        .map(|EdgeKey(p, a, b, src)| Edge::base(p, &[&a, &b], &src))
        .collect()
}

#[derive(PartialEq, Eq, PartialOrd, Ord)]
struct EdgeKey(&'static str, String, String, String);
