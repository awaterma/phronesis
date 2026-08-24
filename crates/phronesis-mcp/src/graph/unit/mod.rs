//! Cargo packages and targets: the namespace root for entity identity.
//!
//! A module path alone does not identify a module. Every crate in a workspace
//! has a root called `crate`, so anchoring identity on the module path alone
//! collapses every member into one namespace. This repository had exactly
//! that: four files across three crates all claiming the module `crate`, and
//! any two same-named modules in different crates (`phronesis::wme` and a
//! future `phronesis-mcp::wme`) would have merged into a single node —
//! inventing import edges between unrelated files and, through them, phantom
//! cycles.
//!
//! Identity is therefore `<lang>:<package>[#<target>]::<module path>`. The
//! target suffix distinguishes a package's library, binaries, integration
//! tests, examples, benchmarks and build script. The language tag is not
//! decoration. Module namespaces from different languages can collide
//! (`utils` is a module in every language there is), and the tag is what keeps
//! one graph sound across several extractors. Carrying it now costs a string
//! prefix; adding it later costs every consumer a rebuild.

mod cargo;
mod node;
mod python;
mod swift;

pub use cargo::parse_cargo_manifest;
pub use node::{parse_package_json, parse_tsconfig};
pub use python::parse_pyproject_manifest;
pub use swift::{SwiftTarget, parse_package_swift};

use node::{index_typescript_files, read_tsconfig_chain};
use python::top_level_packages;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// Language tag for the Rust extractor.
pub const LANG_RUST: &str = "rust";

/// Language tag for the Python extractor.
pub const LANG_PYTHON: &str = "python";

/// Language tag for the TypeScript extractor.
pub const LANG_TYPESCRIPT: &str = "typescript";

/// Language tag for the Lua extractor.
pub const LANG_LUA: &str = "lua";
pub const LANG_RHAI: &str = "rhai";

/// Language tag for the Swift extractor.
pub const LANG_SWIFT: &str = "swift";

/// Language tag for the CUE extractor.
pub const LANG_CUE: &str = "cue";

/// Language tag for the JSON extractor.
pub const LANG_JSON: &str = "json";

/// Language tag for the YAML extractor.
pub const LANG_YAML: &str = "yaml";

/// Language tag for the Helm3 extractor.
pub const LANG_HELM3: &str = "helm3";

/// The language that owns a source file, by extension. A file in no known
/// language belongs to no unit — better to name nothing than to name it under
/// whichever manifest happens to sit nearest.
pub fn lang_of_path(file_rel: &str) -> Option<&'static str> {
    match file_rel.rsplit_once('.') {
        Some((_, "rs")) => Some(LANG_RUST),
        Some((_, "py")) => Some(LANG_PYTHON),
        Some((_, "ts" | "tsx" | "mts" | "cts")) => Some(LANG_TYPESCRIPT),
        Some((_, "swift")) => Some(LANG_SWIFT),
        Some((_, "lua")) => Some(LANG_LUA),
        Some((_, "rhai")) => Some(LANG_RHAI),
        Some((_, "cue")) => Some(LANG_CUE),
        Some((_, "json")) => Some(LANG_JSON),
        Some((_, "yaml" | "yml")) => Some(LANG_YAML),
        Some((_, "tpl")) => Some(LANG_HELM3),
        _ => None,
    }
}

/// Unit name used when no manifest claims a Rust file. Deliberately the same
/// token Rust itself uses for an unnamed root, so a single-crate project with
/// no discoverable `Cargo.toml` still reads naturally.
pub const UNNAMED: &str = "crate";

/// Unit name for a file no manifest claims, in that file's own language.
/// Each language borrows its own word for an unnamed root rather than
/// inheriting Rust's.
fn unnamed_name(lang: &str) -> &'static str {
    match lang {
        LANG_RUST => UNNAMED,
        _ => "project",
    }
}

/// One package — a Cargo package, and later a Python distribution or an npm
/// workspace member. `UnitContext` refines this to a compilation target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unit {
    pub lang: &'static str,
    /// Package name as declared in the manifest.
    pub name: String,
    /// Repo-relative directory containing the manifest; empty at the root.
    pub root: String,
    /// Extern-name -> package name, for dependencies declared under an alias.
    /// `phr = { package = "phronesis" }` means source says `phr::`, and
    /// without this map the edge to `phronesis` is silently dropped.
    pub deps: BTreeMap<String, String>,
    /// Repo-relative directory that import paths are rooted at, for languages
    /// where layout decides it rather than convention. Python's `src/` layout
    /// puts it at `<root>/src`; the flat layout puts it at `<root>`. Unused
    /// for Rust, where each Cargo target computes its own root.
    pub import_root: String,
    /// Top-level import package names this unit provides, as they appear in
    /// source. Python only: a distribution's name need not match the package
    /// it ships (`core-lib` providing `core`), and `import core` names the
    /// package, so the distribution name cannot serve as the key.
    pub packages: Vec<String>,
    /// Resolution rules for this unit. TypeScript only; empty elsewhere.
    pub ts: TsConfig,
    /// Repo-relative TypeScript sources belonging to this unit, sorted.
    /// Resolution is a lookup against this rather than disk I/O, which keeps
    /// the extractor a pure function of its inputs.
    pub files: Vec<String>,
    /// Swift only: this unit is a SwiftPM `.testTarget`, so every file in it
    /// is a test file whatever its name.
    pub test_target: bool,
}

impl Unit {
    /// Library-target namespace prefix for this package, e.g.
    /// `rust:phronesis`.
    pub fn id(&self) -> String {
        format!("{}:{}", self.lang, self.name)
    }
}

/// Everything the extractor needs to name entities in one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitContext {
    /// Namespace prefix, e.g. `rust:phronesis-mcp`.
    pub id: String,
    /// Repo-relative path of this target's crate-root file without its `.rs`,
    /// e.g. `crates/phronesis-mcp/src/lib` or
    /// `crates/phronesis-mcp/benches/graph_sync`. Everything to its left is
    /// already stated by `id`, so the extractor strips it before naming
    /// modules. Empty when no manifest claims the file.
    pub module_base: String,
    /// Extern name as written in source -> unit id of a *sibling in this
    /// project*. Only siblings appear: an edge to a third-party crate we never
    /// scan would be a node with no definitions hanging off it.
    pub siblings: BTreeMap<String, String>,
    /// Resolution rules for this unit. TypeScript only; empty elsewhere.
    pub ts: TsConfig,
    /// Repo-relative TypeScript sources belonging to this unit, sorted.
    pub files: Vec<String>,
    /// Repo-relative Lua sources belonging to this unit, sorted.
    /// Used for resolving `require` calls without disk I/O.
    pub lua_files: Vec<String>,
    /// Repo-relative CUE sources belonging to this unit, sorted.
    /// Used for resolving `import` calls without disk I/O.
    pub cue_files: Vec<String>,
    /// The unit is a test target (SwiftPM `.testTarget`): the extractor types
    /// every file in it `test`, overriding its filename heuristic. False for
    /// every other language and for files no manifest claims.
    pub test_target: bool,
}

impl UnitContext {
    /// Context for a Rust file no manifest claims.
    pub fn unnamed() -> Self {
        Self::unnamed_for(LANG_RUST)
    }

    /// Context for a file of `lang` that no manifest claims.
    ///
    /// The fallback follows the file's own language: naming a stray `.py`
    /// file `rust:crate::…` would merge it with Rust modules of the same
    /// name, making the language tag wrong precisely where the evidence is
    /// thinnest.
    pub fn unnamed_for(lang: &str) -> Self {
        UnitContext {
            id: format!("{lang}:{}", unnamed_name(lang)),
            module_base: String::new(),
            siblings: BTreeMap::new(),
            ts: TsConfig::default(),
            files: Vec::new(),
            lua_files: Vec::new(),
            cue_files: Vec::new(),
            test_target: false,
        }
    }
}

impl Default for UnitContext {
    fn default() -> Self {
        Self::unnamed()
    }
}

/// The parsed shape of one `Cargo.toml`.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Manifest {
    /// `[package] name`, absent for a virtual workspace manifest.
    pub package: Option<String>,
    /// Alias -> package name from this manifest's dependency tables.
    pub deps: BTreeMap<String, String>,
    /// Alias -> package name from `[workspace.dependencies]`, which members
    /// inherit via `dep.workspace = true` and therefore never restate.
    pub workspace_deps: BTreeMap<String, String>,
}

/// The subset of `tsconfig.json` that decides import resolution.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct TsConfig {
    /// `compilerOptions.baseUrl`, relative to the unit root, with no leading
    /// `./` and no trailing `/`. Empty means the unit root itself.
    pub base_url: String,
    /// `compilerOptions.paths`: alias pattern -> replacement patterns, each
    /// keeping its `*` wildcard verbatim so resolution can substitute.
    pub paths: BTreeMap<String, Vec<String>>,
}

/// Every unit in a project, resolvable by file path.
#[derive(Debug, Default, Clone)]
pub struct UnitMap {
    /// Sorted by descending root length, so the first prefix match is the
    /// innermost (and therefore correct) unit for nested workspaces.
    units: Vec<Unit>,
    /// Aliases declared once at the workspace root and inherited by members.
    workspace_deps: BTreeMap<String, String>,
}

impl UnitMap {
    /// Discover every Cargo package under `root`, honouring `.gitignore` so
    /// vendored trees and build output never define units.
    pub fn discover(root: &Path) -> Self {
        let mut manifests = Vec::new();
        let mut workspace_deps = BTreeMap::new();

        for entry in ignore::WalkBuilder::new(root)
            .hidden(true)
            .filter_entry(|e| e.file_name() != "node_modules")
            .build()
            .flatten()
        {
            let lang = match entry.file_name().to_str() {
                Some("Cargo.toml") => LANG_RUST,
                Some("pyproject.toml") => LANG_PYTHON,
                Some("package.json") => LANG_TYPESCRIPT,
                Some("Package.swift") => LANG_SWIFT,
                _ => continue,
            };
            let Ok(text) = std::fs::read_to_string(entry.path()) else {
                continue;
            };
            let dir = rel_dir_of(entry.path(), root);
            if lang == LANG_SWIFT {
                // One manifest, many units: each SwiftPM target is its own
                // module namespace, rooted at the target directory.
                manifests.extend(parse_package_swift(&text).into_iter().map(|t| Unit {
                    lang,
                    name: t.name,
                    root: join_rel(&dir, &t.path),
                    deps: BTreeMap::new(),
                    import_root: String::new(),
                    packages: Vec::new(),
                    ts: TsConfig::default(),
                    files: Vec::new(),
                    test_target: t.is_test,
                }));
                continue;
            }
            let manifest = match lang {
                LANG_RUST => parse_cargo_manifest(&text),
                LANG_PYTHON => parse_pyproject_manifest(&text),
                _ => parse_package_json(&text),
            };
            workspace_deps.extend(manifest.workspace_deps.clone());

            manifests.extend(unit_of_manifest(root, entry.path(), dir, lang, manifest));
        }

        manifests.sort_by(|a, b| b.root.len().cmp(&a.root.len()).then(a.name.cmp(&b.name)));
        UnitMap {
            units: manifests,
            workspace_deps,
        }
    }

    /// Build a map directly, for tests and for callers that already know the
    /// layout.
    pub fn from_units(units: Vec<Unit>) -> Self {
        let mut units = units;
        units.sort_by(|a, b| b.root.len().cmp(&a.root.len()).then(a.name.cmp(&b.name)));
        UnitMap {
            units,
            workspace_deps: BTreeMap::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.units.is_empty()
    }

    /// The innermost unit **of the file's own language** whose root contains
    /// `file_rel`.
    ///
    /// The language filter is not a refinement, it is the point: a repository
    /// can hold a `pyproject.toml` at the root and a `Cargo.toml` under
    /// `crates/`, and innermost-root alone would hand a `.py` file beside the
    /// Rust code a Cargo package — naming it `rust:inner::…` and merging it
    /// with whatever Rust module shares its path.
    ///
    /// TypeScript resolution is membership-based rather than prefix-based: a
    /// file resolves only if it appears in some unit's indexed `files` (see
    /// `Unit::files`), which `discover` populates once from disk. A
    /// `UnitMap` therefore reflects the tree as it stood at `discover` time —
    /// a file created afterward is invisible to this method and to
    /// `context_for` until the map is rediscovered; it falls back to the
    /// unnamed context in the meantime rather than joining the wrong unit or
    /// the right one by accident.
    pub fn resolve(&self, file_rel: &str) -> Option<&Unit> {
        let lang = lang_of_path(file_rel)?;
        self.units.iter().find(|u| {
            if u.lang != lang {
                return false;
            }
            if lang == LANG_TYPESCRIPT {
                // A root-prefix test alone would hand every stray file under
                // the tree — including anything under `node_modules`, which
                // discovery deliberately excludes from both the unit set and
                // the file index — to whichever unit happens to sit at the
                // root. The file index is the authority on membership.
                return u.files.iter().any(|f| f == file_rel);
            }
            u.root.is_empty()
                || file_rel
                    .strip_prefix(&u.root)
                    .is_some_and(|rest| rest.starts_with('/'))
        })
    }

    /// Naming context for one file: its namespace prefix, plus the aliases by
    /// which it can reach other units *in this project*.
    pub fn context_for(&self, file_rel: &str) -> UnitContext {
        let Some(unit) = self.resolve(file_rel) else {
            return UnitContext::unnamed_for(lang_of_path(file_rel).unwrap_or(LANG_RUST));
        };
        let (id, module_base) = target_of(unit, file_rel);
        let known: BTreeSet<&str> = self.units.iter().map(|u| u.name.as_str()).collect();

        if unit.lang == LANG_TYPESCRIPT {
            return UnitContext {
                id,
                module_base: join_rel(&unit.root, &unit.ts.base_url),
                siblings: BTreeMap::new(),
                ts: unit.ts.clone(),
                files: unit.files.clone(),
                lua_files: Vec::new(),
                cue_files: Vec::new(),
                test_target: false,
            };
        }

        let mut siblings = BTreeMap::new();
        if unit.lang == LANG_SWIFT {
            // `import Foo` names a target by its declared name. Only targets
            // of this project appear: `import Foundation` would otherwise
            // hang a node with no definitions off every file.
            for other in self.units.iter().filter(|u| u.lang == LANG_SWIFT) {
                if other.name != unit.name {
                    siblings.insert(other.name.clone(), other.id());
                }
            }
            return UnitContext {
                id,
                module_base,
                siblings,
                ts: TsConfig::default(),
                files: Vec::new(),
                lua_files: Vec::new(),
                cue_files: Vec::new(),
                test_target: unit.test_target,
            };
        }
        if unit.lang == LANG_PYTHON {
            // Every package shipped by another distribution in this project.
            // Third-party imports still resolve to nothing, because nothing
            // outside the project defines a unit.
            for other in self.units.iter().filter(|u| u.lang == LANG_PYTHON) {
                if other.name == unit.name {
                    continue;
                }
                for package in &other.packages {
                    siblings.insert(package.clone(), other.id());
                }
            }
            return UnitContext {
                id,
                module_base,
                siblings,
                ts: TsConfig::default(),
                files: Vec::new(),
                lua_files: Vec::new(),
                cue_files: Vec::new(),
                test_target: false,
            };
        }

        // Workspace-level aliases first so a member's own declaration, which
        // is the more specific statement, overrides them.
        for (alias, package) in self.workspace_deps.iter().chain(unit.deps.iter()) {
            if package != &unit.name && known.contains(package.as_str()) {
                siblings.insert(alias.clone(), format!("{LANG_RUST}:{package}"));
            }
        }
        // Binary, integration-test, example and benchmark targets may import
        // their package's library by its Rust extern name. It is a sibling
        // compilation unit even though Cargo describes both targets in one
        // package.
        if id != unit.id() {
            siblings.insert(unit.name.replace('-', "_"), unit.id());
        }
        UnitContext {
            id,
            module_base,
            siblings,
            ts: TsConfig::default(),
            files: Vec::new(),
            lua_files: Vec::new(),
            cue_files: Vec::new(),
            test_target: false,
        }
    }
}

/// The unit a single-package manifest (`Cargo.toml`, `pyproject.toml`,
/// `package.json`) at `manifest_path` declares, or `None` for a manifest
/// that names no package (a virtual workspace root).
///
/// `dir` is the repo-relative directory holding the manifest.
fn unit_of_manifest(
    root: &Path,
    manifest_path: &Path,
    dir: String,
    lang: &'static str,
    manifest: Manifest,
) -> Option<Unit> {
    let name = manifest.package.clone()?;
    // A `src/` directory beside the manifest means the src layout, where
    // imports are rooted one level deeper. Read from disk rather than
    // guessed: both layouts are current practice and neither is declared in
    // the manifest.
    let import_root = if lang == LANG_PYTHON && manifest_path.with_file_name("src").is_dir() {
        join_rel(&dir, "src")
    } else {
        dir.clone()
    };
    let packages = if lang == LANG_PYTHON {
        top_level_packages(&root.join(&import_root))
    } else {
        Vec::new()
    };
    let (ts, files) = if lang == LANG_TYPESCRIPT {
        let dir_abs = root.join(&dir);
        (
            read_tsconfig_chain(&dir_abs.join("tsconfig.json"), 0),
            index_typescript_files(root, &dir_abs, &dir),
        )
    } else {
        (TsConfig::default(), Vec::new())
    };
    Some(Unit {
        lang,
        name,
        root: dir,
        deps: manifest.deps,
        import_root,
        packages,
        ts,
        files,
        test_target: false,
    })
}

/// Identify the Cargo target that owns `file_rel`, and where its module paths
/// are rooted.
///
/// A package is not a compilation unit: its library, default binary, named
/// binaries, integration tests, examples, benchmarks and build script compile
/// in separate `crate` namespaces. Keeping the target in the graph identity
/// prevents `src/lib.rs` and `src/main.rs` from collapsing back together.
///
/// The second return value is the target's crate-root file minus its `.rs`,
/// repo-relative. The extractor strips it so a module path states only what
/// the identity prefix does not already say.
fn target_of(unit: &Unit, file_rel: &str) -> (String, String) {
    // Python has no compilation targets. Its distribution is the whole
    // namespace, a test module is an ordinary module, and imports are rooted
    // at the layout's import root. Giving it a `#test:` suffix by analogy
    // would split one namespace into several that can never join.
    if unit.lang == LANG_PYTHON {
        return (unit.id(), unit.import_root.clone());
    }
    // A SwiftPM target is already one compilation unit, and its module
    // paths are rooted at the target directory.
    if unit.lang == LANG_SWIFT {
        return (unit.id(), unit.root.clone());
    }
    let rel = if unit.root.is_empty() {
        file_rel
    } else {
        file_rel
            .strip_prefix(&unit.root)
            .and_then(|rest| rest.strip_prefix('/'))
            .unwrap_or(file_rel)
    };
    let base = unit.id();
    // Repo-relative form of a path stated relative to the package root.
    let at = |rel: &str| {
        if unit.root.is_empty() {
            rel.to_string()
        } else {
            format!("{}/{rel}", unit.root)
        }
    };

    if rel == "src/main.rs" {
        return (format!("{base}#bin:{}", unit.name), at("src/main"));
    }
    if let Some(rest) = rel.strip_prefix("src/bin/") {
        let name = first_segment(rest);
        return (format!("{base}#bin:{name}"), at(&format!("src/bin/{name}")));
    }
    for (directory, kind) in [
        ("tests/", "test"),
        ("examples/", "example"),
        ("benches/", "bench"),
    ] {
        if let Some(rest) = rel.strip_prefix(directory) {
            let name = first_segment(rest);
            return (
                format!("{base}#{kind}:{name}"),
                at(&format!("{directory}{name}")),
            );
        }
    }
    if rel == "build.rs" {
        return (format!("{base}#build"), at("build"));
    }
    (base, at("src/lib"))
}

/// Repo-relative directory holding a manifest; empty at the repo root.
fn rel_dir_of(manifest: &Path, root: &Path) -> String {
    manifest
        .parent()
        .and_then(|p| p.strip_prefix(root).ok())
        .and_then(|p| p.to_str())
        .map(|p| p.replace('\\', "/"))
        .unwrap_or_default()
}

/// Join a repo-relative directory to a child name, keeping the repo root
/// spelled as the empty string rather than a leading slash.
///
/// An empty `child` (a unit with no `baseUrl`) must yield `dir` itself, not
/// `dir/` — a trailing slash silently breaks every consumer that joins onto
/// or strips this value (`with_base`, `strip_module_base`), since neither
/// expects a path already ending in `/`.
fn join_rel(dir: &str, child: &str) -> String {
    if dir.is_empty() {
        child.to_string()
    } else if child.is_empty() {
        dir.to_string()
    } else {
        format!("{dir}/{child}")
    }
}

/// The first path component of `rel`, without any `.rs` suffix — the target
/// name, whether the target is one file or a directory of them.
fn first_segment(rel: &str) -> &str {
    let head = rel.split('/').next().unwrap_or(rel);
    head.strip_suffix(".rs").unwrap_or(head)
}

#[cfg(test)]
mod tests;
