//! Maven discovery from local POMs only. No Maven execution or artifact fetch.

mod helper;
mod pom;
mod scope;
mod xml;

pub use scope::Scope;

use super::index::Context;
use pom::{Build, Pom};
use std::collections::{BTreeMap, BTreeSet};

/// Named diagnostics count distinct source objects, independently of skipped
/// Java evidence. Keeping object names permits corpus reconciliation.
#[derive(Debug, Clone, Default)]
pub struct Diagnostics(pub BTreeMap<String, BTreeSet<String>>);

impl Diagnostics {
    pub fn record(&mut self, name: &str, object: impl Into<String>) {
        self.0.entry(name.into()).or_default().insert(object.into());
    }

    pub fn count(&self, name: &str) -> usize {
        self.0.get(name).map_or(0, BTreeSet::len)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct SourceRoot {
    pub path: String,
    pub context: Context,
}

#[derive(Debug, Clone)]
pub struct Unit {
    pub id: String,
    pub root: String,
    pub sources: Vec<SourceRoot>,
    pub dependencies: BTreeMap<String, Scope>,
}

#[derive(Debug, Clone, Default)]
pub struct Discovery {
    pub units: Vec<Unit>,
    pub diagnostics: Diagnostics,
    /// Bad build input must remain stale rather than claim a fresh graph.
    pub failed_manifests: BTreeSet<String>,
    pub excluded_roots: BTreeSet<String>,
}

/// Normalize a path relative to `base`, rejecting escape from the repository.
pub(super) fn normalize(base: &str, path: &str) -> Option<String> {
    if path.starts_with('/') || path.contains('\\') || path.contains(':') {
        return None;
    }
    let mut parts = base
        .split('/')
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>();
    for part in path.split('/') {
        match part {
            "." | "" => {}
            ".." => {
                parts.pop()?;
            }
            other => parts.push(other),
        }
    }
    Some(parts.join("/"))
}

fn directory(file: &str) -> &str {
    file.rsplit_once('/').map_or("", |(directory, _)| directory)
}

#[derive(Debug, Clone, Default)]
struct Effective {
    group: Option<String>,
    build: Build,
    dependencies: BTreeMap<(String, String), Scope>,
}

fn effective(
    file: &str,
    poms: &BTreeMap<String, Pom>,
    cache: &mut BTreeMap<String, Effective>,
    visiting: &mut BTreeSet<String>,
    diagnostics: &mut Diagnostics,
) -> Effective {
    if let Some(found) = cache.get(file) {
        return found.clone();
    }
    if !visiting.insert(file.into()) {
        diagnostics.record("parent_cycle", file);
        return Effective::default();
    }
    let Some(pom) = poms.get(file) else {
        return Effective::default();
    };
    let mut inherited = Effective::default();
    if let Some(parent) = &pom.parent {
        let path = parent.path.as_deref().unwrap_or("../pom.xml");
        let normalized = (!path.is_empty())
            .then(|| normalize(directory(file), path))
            .flatten();
        let candidate = normalized.and_then(|path| {
            if poms.contains_key(&path) {
                Some(path)
            } else {
                let path = format!("{path}/pom.xml");
                poms.contains_key(&path).then_some(path)
            }
        });
        if let Some(path) = candidate {
            let config = effective(&path, poms, cache, visiting, diagnostics);
            let matches_artifact = parent.artifact.as_ref().is_none_or(|artifact| {
                poms.get(&path).and_then(|pom| pom.artifact.as_ref()) == Some(artifact)
            });
            let matches_group = parent
                .group
                .as_ref()
                .is_none_or(|group| config.group.as_ref() == Some(group));
            if matches_artifact && matches_group {
                inherited = config;
                inherited.build.helper.inherit();
            } else {
                diagnostics.record("external_parent_skipped", file);
            }
        } else {
            diagnostics.record("external_parent_skipped", file);
        }
        // Parent coordinates remain useful even when its POM is external.
        inherited.group = parent.group.clone().or(inherited.group);
    }
    inherited.group = pom.group.clone().or(inherited.group);
    inherited.build.merge(&pom.build);
    inherited.dependencies.extend(pom.dependencies.clone());
    visiting.remove(file);
    cache.insert(file.into(), inherited.clone());
    inherited
}

fn interpolate(value: &str, group: &str, artifact: &str) -> String {
    value
        .replace("${project.groupId}", group)
        .replace("${project.artifactId}", artifact)
}

fn resolve_root(root: &str, value: &str, group: &str, artifact: &str) -> Option<String> {
    let value = interpolate(value, group, artifact);
    if value.contains("${") && !value.contains("${project.basedir}") {
        return None;
    }
    // Interpret basedir as an anchored repository-relative path, not as a
    // second copy of the unit prefix. No absolute host paths enter identities.
    if let Some(suffix) = value.strip_prefix("${project.basedir}") {
        if !suffix.contains("${") && (suffix.is_empty() || suffix.starts_with('/')) {
            return normalize(root, suffix.trim_start_matches('/'));
        }
        return None;
    }
    if value.contains("${") {
        return None;
    }
    normalize(root, &value)
}

fn versioned_root(path: &str) -> bool {
    path.rsplit('/').next().is_some_and(|last| {
        last.strip_prefix("java")
            .is_some_and(|suffix| !suffix.is_empty() && suffix.bytes().all(|b| b.is_ascii_digit()))
    })
}

fn source_roots(
    file: &str,
    config: &Effective,
    group: &str,
    artifact: &str,
    out: &mut Discovery,
) -> Vec<SourceRoot> {
    let root = directory(file);
    let mut paths = Vec::new();
    for (context, configured, default) in [
        (
            Context::Production,
            &config.build.production,
            "src/main/java",
        ),
        (Context::Test, &config.build.test, "src/test/java"),
    ] {
        let value = configured.as_deref().unwrap_or(default);
        let path = resolve_root(root, value, group, artifact).or_else(|| {
            out.diagnostics
                .record("unresolved_property", format!("{file}:{value}"));
            normalize(root, default)
        });
        if let Some(path) = path {
            paths.push(SourceRoot { path, context });
        }
    }
    for (context, value) in config.build.helper.roots() {
        if let Some(path) = resolve_root(root, &value, group, artifact) {
            paths.push(SourceRoot { path, context });
        } else {
            out.diagnostics
                .record("unresolved_property", format!("{file}:{value}"));
        }
    }
    paths.retain(|source| {
        if versioned_root(&source.path) {
            out.diagnostics
                .record("multi_release_root_ignored", &source.path);
            out.excluded_roots.insert(source.path.clone());
            false
        } else {
            true
        }
    });
    paths.sort();
    paths.dedup();
    paths
}

/// Discover units from repository-relative manifest paths and their contents.
/// The caller's ignore-aware walk controls which files are repository inputs.
pub fn discover(manifests: &BTreeMap<String, String>) -> Discovery {
    let mut out = Discovery::default();
    let mut poms = BTreeMap::new();
    for (file, body) in manifests {
        match pom::parse(file, body, &mut out.diagnostics) {
            Some(pom) => {
                poms.insert(file.clone(), pom);
            }
            None => {
                out.failed_manifests.insert(file.clone());
            }
        }
    }
    let mut cache = BTreeMap::new();
    let mut ids = BTreeSet::new();
    for (file, pom) in &poms {
        let config = effective(
            file,
            &poms,
            &mut cache,
            &mut BTreeSet::new(),
            &mut out.diagnostics,
        );
        if pom.packaging.as_deref() == Some("pom") {
            continue;
        }
        let Some(artifact) = pom.artifact.as_deref().filter(|a| !a.contains("${")) else {
            out.diagnostics.record("unit_identity_unresolved", file);
            continue;
        };
        let group = config
            .group
            .as_deref()
            .filter(|g| !g.contains("${"))
            .unwrap_or("");
        let id = if group.is_empty() {
            out.diagnostics.record("group_id_unresolved", file);
            artifact.to_string()
        } else {
            format!("{group}:{artifact}")
        };
        if !ids.insert(id.clone()) {
            out.diagnostics.record("unit_id_collision", file);
            continue;
        }
        let sources = source_roots(file, &config, group, artifact, &mut out);
        let mut dependencies = BTreeMap::new();
        for ((dep_group, dep_artifact), scope) in &config.dependencies {
            let target = interpolate(&format!("{dep_group}:{dep_artifact}"), group, artifact);
            if target.contains("${") {
                out.diagnostics
                    .record("unresolved_property", format!("{file}:{target}"));
            } else {
                dependencies.insert(target, *scope);
            }
        }
        out.units.push(Unit {
            id,
            root: directory(file).into(),
            sources,
            dependencies,
        });
    }
    out.units
        .sort_by(|a, b| b.root.len().cmp(&a.root.len()).then(a.id.cmp(&b.id)));
    out
}

impl Discovery {
    /// Reactor units on this unit's production or test compile classpath.
    /// Propagate scopes using Maven's table, retaining runtime paths while
    /// traversing because test compilation can see them.
    pub fn visible_units(&self, source: &str, test: bool) -> BTreeSet<String> {
        let units = self
            .units
            .iter()
            .map(|u| (u.id.as_str(), u))
            .collect::<BTreeMap<_, _>>();
        let Some(unit) = units.get(source) else {
            return BTreeSet::new();
        };
        let mut pending = unit
            .dependencies
            .iter()
            .map(|(id, scope)| (id.clone(), *scope))
            .collect::<Vec<_>>();
        let mut visited = BTreeSet::new();
        let mut visible = BTreeSet::new();
        while let Some((id, scope)) = pending.pop() {
            if !visited.insert((id.clone(), scope)) {
                continue;
            }
            let Some(target) = units.get(id.as_str()) else {
                continue;
            };
            if scope.on_compile_classpath(test) {
                visible.insert(id.clone());
            }
            for (next, child_scope) in &target.dependencies {
                if let Some(scope) = scope.transitive(*child_scope) {
                    pending.push((next.clone(), scope));
                }
            }
        }
        visible.remove(source);
        visible
    }
}

#[cfg(test)]
mod tests;
