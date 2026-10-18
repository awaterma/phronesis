//! Pack static test selection helpers.
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use super::{PackError, PackRegistry, TestCommand, extract, file_context, graph_records};
use crate::coverage::region_map::function_region_id;
use crate::coverage::select::FileDiff;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackChangedSymbol {
    pub file: String,
    pub symbol: String,
    pub region: String,
}

/// Compare old and new declaration spans, not merely function names. Removed
/// symbols are returned but must be joined against live graph declarations.
pub fn changed_pack_symbols(
    registry: &PackRegistry,
    root: &Path,
    diffs: &[FileDiff],
) -> Result<Vec<PackChangedSymbol>, PackError> {
    let mut changed = BTreeSet::new();
    for diff in diffs {
        let Some(pack) = registry.for_path(&diff.path)? else {
            continue;
        };
        let context = file_context(root, &diff.path, pack)?;
        let declarations = |source: &str| -> Result<BTreeMap<String, Vec<String>>, PackError> {
            if source.is_empty() {
                return Ok(BTreeMap::new());
            }
            let extraction = extract(pack, &context, source)?;
            if !extraction.diagnostics.is_empty() {
                return Err(PackError::Record {
                    pack: Some(pack.assets.manifest.id.clone()),
                    path: Some(diff.path.clone()),
                    message: format!("{} adapter diagnostic(s)", extraction.diagnostics.len()),
                });
            }
            let edges = graph_records(pack, &context, &extraction)?;
            let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
            for edge in edges
                .iter()
                .filter(|e| e.p == "pack_declaration" && e.a.len() >= 8)
            {
                let start = edge.a[6].parse::<usize>().map_err(|e| PackError::Record {
                    pack: Some(pack.assets.manifest.id.clone()),
                    path: Some(diff.path.clone()),
                    message: e.to_string(),
                })?;
                let end = edge.a[7].parse::<usize>().map_err(|e| PackError::Record {
                    pack: Some(pack.assets.manifest.id.clone()),
                    path: Some(diff.path.clone()),
                    message: e.to_string(),
                })?;
                let body = source.get(start..end).ok_or_else(|| PackError::Record {
                    pack: Some(pack.assets.manifest.id.clone()),
                    path: Some(diff.path.clone()),
                    message: "invalid declaration span".into(),
                })?;
                out.entry(edge.a[1].clone())
                    .or_default()
                    .push(body.to_string());
            }
            for bodies in out.values_mut() {
                bodies.sort();
            }
            Ok(out)
        };
        let old = declarations(diff.old.as_deref().unwrap_or(""))?;
        let new = declarations(&diff.new)?;
        for symbol in old.keys().chain(new.keys()) {
            if old.get(symbol) != new.get(symbol) {
                changed.insert((diff.path.clone(), symbol.clone()));
            }
        }
    }
    Ok(changed
        .into_iter()
        .map(|(file, symbol)| PackChangedSymbol {
            region: function_region_id(&file, &symbol),
            file,
            symbol,
        })
        .collect())
}

fn shell_word(word: &str) -> Result<String, PackError> {
    if word.contains('\0') {
        return Err(PackError::Record {
            pack: None,
            path: None,
            message: "test command contains NUL".into(),
        });
    }
    Ok(format!("'{}'", word.replace('\'', "'\\''")))
}

/// Render from the repository root. Every adapter-supplied word stays one
/// argument even if it contains quotes, Unicode, or shell metacharacters.
pub fn render_test_command(command: &TestCommand) -> Result<String, PackError> {
    if command.program.is_empty()
        || (command.cwd != "." && super::relative_path_problem(&command.cwd).is_some())
    {
        return Err(PackError::Record {
            pack: None,
            path: None,
            message: "invalid test command program or cwd".into(),
        });
    }
    let mut invocation = shell_word(&command.program)?;
    for arg in &command.args {
        invocation.push(' ');
        invocation.push_str(&shell_word(arg)?);
    }
    if command.cwd == "." {
        Ok(invocation)
    } else {
        Ok(format!(
            "(cd -- {} && {})",
            shell_word(&command.cwd)?,
            invocation
        ))
    }
}
