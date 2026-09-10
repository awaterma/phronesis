//! Bazel package ownership and per-file compile visibility.

mod eval;

use super::index::Context;
use super::maven::Diagnostics;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct File {
    pub unit: String,
    pub context: Context,
    pub visible_files: Arc<BTreeSet<String>>,
}

#[derive(Debug, Clone, Default)]
pub struct Discovery {
    pub files: BTreeMap<String, File>,
    pub diagnostics: Diagnostics,
    /// Launch entry points are diagnostic inputs; they never classify files.
    pub test_entry_points: Vec<(String, String)>,
}

#[derive(Debug)]
struct Target {
    sources: BTreeSet<String>,
    deps: Vec<String>,
    exports: Vec<String>,
    container: bool,
    test: bool,
    alias: Option<Vec<String>>,
}

fn directory(file: &str) -> &str {
    file.rsplit_once('/').map_or("", |(dir, _)| dir)
}

fn relative<'a>(package: &str, file: &'a str) -> Option<&'a str> {
    if package.is_empty() {
        Some(file)
    } else {
        file.strip_prefix(package)?.strip_prefix('/')
    }
}

fn join(package: &str, file: &str) -> String {
    if package.is_empty() {
        file.into()
    } else {
        format!("{package}/{file}")
    }
}

fn label(package: &str, name: &str) -> Option<String> {
    if name.starts_with('@') {
        return None;
    }
    if let Some(local) = name.strip_prefix(':') {
        return (!local.is_empty()).then(|| format!("//{package}:{local}"));
    }
    if let Some(full) = name.strip_prefix("//") {
        return if full.contains(':') {
            Some(name.into())
        } else {
            let target = full.rsplit('/').next()?;
            (!target.is_empty()).then(|| format!("{name}:{target}"))
        };
    }
    (!name.is_empty()
        && !name.starts_with('/')
        && !name.contains(':')
        && !name.split('/').any(|part| matches!(part, "" | "." | "..")))
    .then(|| format!("//{package}:{name}"))
}

fn sources(
    name: &str,
    calls: &BTreeMap<String, eval::Call>,
    files: &BTreeSet<String>,
    visiting: &mut BTreeSet<String>,
    build: &str,
    diagnostics: &mut Diagnostics,
) -> BTreeSet<String> {
    if !visiting.insert(name.into()) {
        diagnostics.record("unresolved_label", format!("{build}:{name}"));
        return BTreeSet::new();
    }
    let mut out = BTreeSet::new();
    if let Some(call) = calls.get(name) {
        for source in &call.srcs {
            let local = source.strip_prefix(':').unwrap_or(source);
            if files.contains(local) {
                out.insert(local.into());
            } else if calls
                .get(local)
                .is_some_and(|call| call.kind == "filegroup")
            {
                out.extend(sources(local, calls, files, visiting, build, diagnostics));
            } else {
                diagnostics.record("unresolved_label", format!("{build}:{source}"));
            }
        }
    }
    visiting.remove(name);
    out
}

/// Collect the exports of a dependency, without following its ordinary deps.
fn exported_files(
    label: &str,
    targets: &BTreeMap<String, Target>,
    visited: &mut BTreeSet<String>,
    active: &mut BTreeSet<String>,
    files: &mut BTreeSet<String>,
    diagnostics: &mut Diagnostics,
) {
    if active.contains(label) {
        diagnostics.record("unresolved_label", label);
        return;
    }
    if !visited.insert(label.into()) {
        return;
    }
    let Some(target) = targets.get(label) else {
        diagnostics.record("unresolved_label", label);
        return;
    };
    active.insert(label.into());
    if let Some(alternatives) = &target.alias {
        for actual in alternatives {
            exported_files(actual, targets, visited, active, files, diagnostics);
        }
        active.remove(label);
        return;
    }
    if target.container {
        diagnostics.record("unresolved_label", label);
        active.remove(label);
        return;
    }
    files.extend(target.sources.clone());
    for export in &target.exports {
        exported_files(export, targets, visited, active, files, diagnostics);
    }
    active.remove(label);
}

/// Discover from an ignore-aware snapshot: BUILD paths and all Java paths.
/// The longest BUILD directory prefix defines the package boundary used by
/// both literal ownership and globs, independent of traversal/target order.
pub fn discover(builds: &BTreeMap<String, String>, java_files: &[String]) -> Discovery {
    let mut out = Discovery::default();
    let mut packages = BTreeMap::new();
    for (file, body) in builds {
        let package = directory(file).to_string();
        if !packages.contains_key(&package) || file.ends_with("BUILD.bazel") {
            packages.insert(package, (file, body));
        }
    }
    let mut targets = BTreeMap::new();
    let mut claims: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (package, (build, body)) in &packages {
        let files = java_files
            .iter()
            .filter(|file| {
                relative(package, file).is_some()
                    && !packages.keys().any(|nested| {
                        nested.len() > package.len() && relative(nested, file).is_some()
                    })
            })
            .filter_map(|file| relative(package, file).map(str::to_string))
            .collect::<BTreeSet<_>>();
        for file in &files {
            out.files.insert(
                join(package, file),
                File {
                    unit: format!("//{package}"),
                    context: Context::Unclaimed,
                    visible_files: Arc::default(),
                },
            );
        }
        let evaluated = eval::evaluate(
            build,
            body,
            &files.iter().cloned().collect::<Vec<_>>(),
            &mut out.diagnostics,
        );
        let mut calls = BTreeMap::new();
        for (i, call) in evaluated.into_iter().enumerate() {
            let name = if call.name.is_empty() {
                format!("<anonymous:{i}>")
            } else {
                call.name.clone()
            };
            if calls.contains_key(&name) {
                out.diagnostics
                    .record("duplicate_target", format!("{build}:{name}"));
                continue;
            }
            calls.insert(name, call);
        }
        for (name, call) in &calls {
            if let Some(entry_point) = &call.test_class {
                out.test_entry_points
                    .push((format!("//{package}:{name}"), entry_point.clone()));
            }
            let id = format!("//{package}:{name}");
            let sources = sources(
                name,
                &calls,
                &files,
                &mut BTreeSet::new(),
                build,
                &mut out.diagnostics,
            )
            .into_iter()
            .map(|file| join(package, &file))
            .collect::<BTreeSet<_>>();
            let container = call.kind == "filegroup";
            if !container {
                for source in &sources {
                    claims.entry(source.clone()).or_default().push(id.clone());
                }
            }
            let mut resolve_labels = |labels: &[String]| {
                labels
                    .iter()
                    .filter_map(|name| {
                        let resolved = label(package, name);
                        if resolved.is_none() {
                            out.diagnostics
                                .record("unresolved_label", format!("{build}:{name}"));
                        }
                        resolved
                    })
                    .collect::<Vec<_>>()
            };
            targets.insert(
                id,
                Target {
                    sources,
                    deps: resolve_labels(&call.deps),
                    exports: resolve_labels(&call.exports),
                    container,
                    test: call.kind.ends_with("_test"),
                    alias: if call.kind == "alias" {
                        Some(resolve_labels(&call.actual))
                    } else {
                        None
                    },
                },
            );
        }
    }
    let mut visibility = BTreeMap::new();
    for (file, metadata) in &mut out.files {
        let Some(claiming) = claims.get(file) else {
            out.diagnostics.record("files_unclaimed", file);
            continue;
        };
        let production = claiming
            .iter()
            .any(|id| targets.get(id).is_some_and(|target| !target.test));
        let test = claiming
            .iter()
            .any(|id| targets.get(id).is_some_and(|target| target.test));
        metadata.context = if production {
            Context::Production
        } else {
            Context::Test
        };
        if production && test {
            out.diagnostics.record("dual_claimed_file", file);
        }
        if claiming.len() > 1 {
            out.diagnostics
                .record("multi_target_visibility_unioned", file);
        }
        if let Some(shared) = visibility.get(claiming) {
            metadata.visible_files = Arc::clone(shared);
            continue;
        }
        let mut visible_files = BTreeSet::new();
        for id in claiming {
            let Some(target) = targets.get(id) else {
                continue;
            };
            visible_files.extend(target.sources.iter().cloned());
            for dependency in target.deps.iter().chain(&target.exports) {
                exported_files(
                    dependency,
                    &targets,
                    &mut BTreeSet::new(),
                    &mut BTreeSet::new(),
                    &mut visible_files,
                    &mut out.diagnostics,
                );
            }
        }
        metadata.visible_files = Arc::new(visible_files);
        visibility.insert(claiming.clone(), Arc::clone(&metadata.visible_files));
    }
    out
}

#[cfg(test)]
mod tests;
