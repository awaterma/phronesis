//! One repository-wide Java declaration and build snapshot per graph pass.

mod cache;
mod extract;

use super::index::{Context, DeclarationIndex, Owner, module_id};
use super::{bazel, maven, parse};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct File {
    pub owner: Owner,
    pub source: Arc<parse::Source>,
    pub visible: BTreeSet<String>,
    pub path_mismatch: bool,
}

#[derive(Debug, Clone, Default)]
pub struct Project {
    pub files: BTreeMap<String, File>,
    pub index: DeclarationIndex,
    pub diagnostics: maven::Diagnostics,
    pub input_hashes: BTreeMap<String, u64>,
    pub failed_inputs: BTreeSet<String>,
}

pub fn is_manifest(file: &str) -> bool {
    matches!(
        file.rsplit('/').next(),
        Some("pom.xml" | "BUILD" | "BUILD.bazel")
    )
}

/// Normalize an overlay within the discovery walk's path boundary.
/// Resolve the nearest existing ancestor to support files not yet on disk.
pub(crate) fn overlay_path(root: &Path, file: &str) -> Option<String> {
    let relative = crate::graph::hydrate::repo_relative(root, file)?;
    if Path::new(&relative).components().any(|part| {
        !matches!(part, std::path::Component::Normal(_))
            || part.as_os_str() == "node_modules"
            || part.as_os_str().to_string_lossy().starts_with('.')
    }) {
        return None;
    }
    let canonical_root = root.canonicalize().ok()?;
    let candidate = root.join(&relative);
    let existing = candidate.ancestors().find(|path| path.exists())?;
    if !existing.canonicalize().ok()?.starts_with(canonical_root) {
        return None;
    }
    Some(relative)
}

/// The Java snapshot uses the same hidden-directory and node_modules policy
/// as graph tracking. Source files and build metadata are separate inputs.
pub fn input_files(root: &Path) -> Vec<String> {
    let mut files = ignore::WalkBuilder::new(root)
        .hidden(true)
        .filter_entry(|e| e.file_name() != "node_modules")
        .build()
        .flatten()
        .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
        .filter_map(|e| {
            e.path()
                .strip_prefix(root)
                .ok()?
                .to_str()
                .map(|s| s.replace('\\', "/"))
        })
        .filter(|file| file.ends_with(".java") || is_manifest(file))
        .collect::<Vec<_>>();
    files.sort();
    files
}

fn under(root: &str, file: &str) -> bool {
    root.is_empty()
        || file
            .strip_prefix(root)
            .is_some_and(|rest| rest.starts_with('/'))
}

fn directory(file: &str) -> &str {
    file.rsplit_once('/').map_or("", |(dir, _)| dir)
}

fn versioned_path(file: &str) -> Option<String> {
    let mut prefix = Vec::new();
    for part in file.split('/') {
        prefix.push(part);
        if prefix.len() >= 3
            && prefix[prefix.len() - 3] == "src"
            && matches!(prefix[prefix.len() - 2], "main" | "test")
            && part.strip_prefix("java").is_some_and(|suffix| {
                !suffix.is_empty() && suffix.bytes().all(|b| b.is_ascii_digit())
            })
        {
            return Some(prefix.join("/"));
        }
    }
    None
}

fn bazel_source_root(unit: &str, file: &str) -> String {
    let package = unit.strip_prefix("//").unwrap_or("");
    // BUILD files may sit deep inside a declared Java package. A conventional
    // source root above that BUILD still governs the package/path cross-check.
    for candidate in ["src/main/java", "src/test/java", "javatests", "java"] {
        let pattern = format!("{candidate}/");
        if let Some((offset, _)) = file
            .match_indices(&pattern)
            .find(|(offset, _)| *offset == 0 || file.as_bytes()[offset - 1] == b'/')
        {
            return file[..offset + candidate.len()].into();
        }
    }
    for candidate in ["src/main/java", "src/test/java", "javatests", "java", "src"] {
        let root = if package.is_empty() {
            candidate.into()
        } else {
            format!("{package}/{candidate}")
        };
        if under(&root, file) {
            return root;
        }
    }
    package.into()
}

impl Project {
    /// Read current bytes, optionally overlaying one unsaved source/manifest.
    /// This never writes the caller's supplied content to disk.
    pub fn discover(root: &Path, overlay: Option<(&str, &str)>) -> std::io::Result<Self> {
        let mut inputs = BTreeMap::new();
        let mut unreadable = BTreeSet::new();
        for file in input_files(root) {
            match std::fs::read_to_string(root.join(&file)) {
                Ok(body) => {
                    inputs.insert(file, body);
                }
                Err(_) => {
                    unreadable.insert(file.clone());
                    inputs.insert(file, String::new());
                }
            }
        }
        if let Some((file, content)) = overlay
            && let Some(file) = overlay_path(root, file)
            && (file.ends_with(".java") || is_manifest(&file))
        {
            unreadable.remove(&file);
            inputs.insert(file, content.into());
        }
        let mut parsed = cache::parse_sources(root, &inputs);
        for file in unreadable.iter().filter(|file| file.ends_with(".java")) {
            parsed.insert(
                file.clone(),
                Arc::new(parse::Source {
                    parse_failed: true,
                    skipped: 1,
                    ..parse::Source::default()
                }),
            );
        }
        let mut project = Self::assemble(&inputs, parsed);
        for file in unreadable {
            project.input_hashes.remove(&file);
            project.diagnostics.record("unreadable_input", &file);
            project.failed_inputs.insert(file);
        }
        Ok(project)
    }

    /// Pure construction for callers with an existing input snapshot.
    pub fn from_inputs(inputs: &BTreeMap<String, String>) -> Self {
        let parsed = inputs
            .iter()
            .filter(|(file, _)| file.ends_with(".java"))
            .map(|(file, body)| (file.clone(), Arc::new(parse::parse(file, body))))
            .collect();
        Self::assemble(inputs, parsed)
    }

    fn assemble(
        inputs: &BTreeMap<String, String>,
        parsed: BTreeMap<String, Arc<parse::Source>>,
    ) -> Self {
        let maven_inputs = inputs
            .iter()
            .filter(|(file, _)| file.rsplit('/').next() == Some("pom.xml"))
            .map(|(file, body)| (file.clone(), body.clone()))
            .collect();
        let bazel_inputs = inputs
            .iter()
            .filter(|(file, _)| matches!(file.rsplit('/').next(), Some("BUILD" | "BUILD.bazel")))
            .map(|(file, body)| (file.clone(), body.clone()))
            .collect();
        let maven = maven::discover(&maven_inputs);
        let bazel = bazel::discover(&bazel_inputs, &parsed.keys().cloned().collect::<Vec<_>>());
        let mut out = Self {
            diagnostics: maven.diagnostics.clone(),
            failed_inputs: maven.failed_manifests.clone(),
            ..Self::default()
        };
        for (name, objects) in &bazel.diagnostics.0 {
            out.diagnostics
                .0
                .entry(name.clone())
                .or_default()
                .extend(objects.clone());
        }
        for (file, body) in inputs {
            out.input_hashes
                .insert(file.clone(), crate::graph::sync::hash_content(body));
        }
        let mut backends = BTreeMap::new();
        for (file, source) in parsed {
            for (i, import) in source.imports.iter().enumerate() {
                if matches!(import, parse::ImportDecl::Module) {
                    out.diagnostics
                        .record("module_import_ignored", format!("{file}:{i}"));
                }
            }
            if source.module_info {
                out.diagnostics.record("module_info_skipped", &file);
                // The nearest discovered unit is reported, even though JPMS
                // descriptors themselves never create a default-package node.
                let unit = maven
                    .units
                    .iter()
                    .find(|u| under(&u.root, &file))
                    .map(|u| u.id.clone())
                    .or_else(|| bazel.files.get(&file).map(|f| f.unit.clone()))
                    .unwrap_or_else(|| "project".into());
                out.diagnostics.record("jpms_unit_unmodelled", unit);
                continue;
            }
            if let Some(versioned) = versioned_path(&file) {
                out.diagnostics
                    .record("multi_release_root_ignored", versioned);
                continue;
            }
            if maven.excluded_roots.iter().any(|root| under(root, &file)) {
                continue;
            }
            let maven_owner = maven.units.iter().find_map(|unit| {
                unit.sources
                    .iter()
                    .filter(|root| under(&root.path, &file))
                    .min_by_key(|root| (root.context, std::cmp::Reverse(root.path.len())))
                    .map(|root| (unit, root))
            });
            let bazel_owner = bazel.files.get(&file);
            let use_maven = maven_owner.is_some_and(|(unit, _)| {
                bazel_owner.is_none_or(|b| unit.root.len() >= b.unit.trim_start_matches("//").len())
            });
            let (unit, context, source_root, backend) = if use_maven {
                let Some((unit, root)) = maven_owner else {
                    continue;
                };
                (unit.id.clone(), root.context, root.path.clone(), "maven")
            } else if let Some(owner) = bazel_owner {
                (
                    owner.unit.clone(),
                    owner.context,
                    bazel_source_root(&owner.unit, &file),
                    "bazel",
                )
            } else {
                (
                    "project".into(),
                    Context::Production,
                    String::new(),
                    "fallback",
                )
            };
            if maven_owner.is_some() && bazel_owner.is_some() {
                out.diagnostics.record("mixed_build_owner_selected", &file);
            }
            let owner = Owner {
                module: module_id(&unit, &source.package),
                unit,
                context,
                file: file.clone(),
            };
            if source.parse_failed {
                out.failed_inputs.insert(file.clone());
            } else {
                out.index
                    .insert(&source.package, owner.clone(), &source.types);
            }
            let expected = source.package.replace('.', "/");
            let actual_dir = directory(&file);
            let actual = if source_root.is_empty() {
                actual_dir
            } else {
                actual_dir
                    .strip_prefix(&source_root)
                    .unwrap_or(actual_dir)
                    .trim_start_matches('/')
            };
            let path_mismatch = actual != expected;
            out.files.insert(
                file.clone(),
                File {
                    owner,
                    source,
                    visible: BTreeSet::new(),
                    path_mismatch,
                },
            );
            backends.insert(file, backend);
        }
        let owners = out
            .files
            .iter()
            .map(|(file, entry)| (file.clone(), entry.owner.clone()))
            .collect::<BTreeMap<_, _>>();
        let mut classpaths = BTreeMap::new();
        for (file, entry) in &mut out.files {
            match backends[file] {
                "bazel" => {
                    if let Some(metadata) = bazel.files.get(file) {
                        entry.visible = metadata.visible_files.clone();
                    }
                }
                backend => {
                    let test = entry.owner.context == Context::Test;
                    let visible_units = classpaths
                        .entry((entry.owner.unit.clone(), test))
                        .or_insert_with(|| {
                            if backend == "maven" {
                                maven.visible_units(&entry.owner.unit, test)
                            } else {
                                BTreeSet::new()
                            }
                        });
                    for (candidate_file, candidate) in &owners {
                        if (candidate.unit == entry.owner.unit
                            && (test || candidate.context != Context::Test))
                            || (visible_units.contains(&candidate.unit)
                                && candidate.context == Context::Production)
                        {
                            entry.visible.insert(candidate_file.clone());
                        }
                    }
                }
            }
        }
        for (target, class) in bazel.test_entry_points {
            let found = out
                .files
                .values()
                .filter(|entry| {
                    entry.source.types.iter().any(|name| {
                        let fqn = if entry.source.package.is_empty() {
                            name.clone()
                        } else {
                            format!("{}.{}", entry.source.package, name)
                        };
                        fqn == class
                    })
                })
                .count()
                == 1;
            out.diagnostics.record(
                if found {
                    "test_class_entry_point"
                } else {
                    "test_class_unresolved"
                },
                target,
            );
        }
        let mut packages: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
        for file in out.files.values() {
            packages
                .entry(&file.source.package)
                .or_default()
                .insert(&file.owner.unit);
        }
        for (package, units) in packages {
            if units.len() > 1 {
                out.diagnostics.record("split_package", package);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests;
