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

use regex::Regex;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::LazyLock;

/// Language tag for the Rust extractor.
pub const LANG_RUST: &str = "rust";

/// Unit name used when no manifest claims a file. Deliberately the same token
/// Rust itself uses for an unnamed root, so a single-crate project with no
/// discoverable `Cargo.toml` still reads naturally.
pub const UNNAMED: &str = "crate";

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
}

impl UnitContext {
    /// Context for a file no manifest claims.
    pub fn unnamed() -> Self {
        UnitContext {
            id: format!("{LANG_RUST}:{UNNAMED}"),
            module_base: String::new(),
            siblings: BTreeMap::new(),
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

/// `package = "x"` inside an inline table, anchored so that keys merely
/// *ending* in `package` (`default-package`) do not match.
static PACKAGE_KEY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?:^|[{,\s])package\s*=\s*"([^"]+)""#).expect("static regex compiles")
});

/// Drop a trailing `#` comment, respecting quoted strings so a `#` inside a
/// path or version string is not mistaken for one.
fn strip_comment(line: &str) -> &str {
    let mut in_quotes = false;
    for (i, c) in line.char_indices() {
        match c {
            '"' => in_quotes = !in_quotes,
            '#' if !in_quotes => return &line[..i],
            _ => {}
        }
    }
    line
}

/// Net brace depth contributed by a line, ignoring braces inside strings.
fn depth_delta(s: &str) -> i32 {
    let mut in_quotes = false;
    let mut depth = 0;
    for c in s.chars() {
        match c {
            '"' => in_quotes = !in_quotes,
            '{' if !in_quotes => depth += 1,
            '}' if !in_quotes => depth -= 1,
            _ => {}
        }
    }
    depth
}

fn unquote(s: &str) -> &str {
    s.trim().trim_matches('"')
}

/// Which alias map a section header feeds, if any.
enum DepTable {
    /// `[dependencies]`, `[dev-dependencies]`, `[target.'…'.dependencies]`
    Local,
    /// `[workspace.dependencies]`
    Workspace,
}

fn dep_table(section: &str) -> Option<DepTable> {
    if section == "workspace.dependencies" || section.starts_with("workspace.dependencies.") {
        return Some(DepTable::Workspace);
    }
    let is_local = section == "dependencies"
        || section == "dev-dependencies"
        || section == "build-dependencies"
        || section.ends_with(".dependencies")
        || section.starts_with("dependencies.")
        || section.starts_with("dev-dependencies.")
        || section.starts_with("build-dependencies.");
    is_local.then_some(DepTable::Local)
}

/// Parse the subset of `Cargo.toml` that bears on identity: the package name
/// and the dependency aliases.
///
/// Hand-written rather than pulling in a TOML parser, because the subset is
/// small and stable: a section header, `name = "…"`, and `package = "…"`
/// inside a dependency's inline table. Anything it fails to understand
/// degrades to "no alias", which loses an edge rather than inventing one.
pub fn parse_cargo_manifest(text: &str) -> Manifest {
    let mut out = Manifest::default();
    let mut section = String::new();
    // An inline table can span lines; accumulate until the braces balance.
    let mut pending: Option<(String, String, i32)> = None;

    for raw in text.lines() {
        let line = strip_comment(raw).trim();

        if let Some((_, body, depth)) = pending.as_mut() {
            body.push(' ');
            body.push_str(line);
            *depth += depth_delta(line);
            if *depth <= 0 {
                let (alias, body, _) = pending.take().unwrap_or_default();
                record_dep(&mut out, &section, alias, &body);
            }
            continue;
        }

        if line.starts_with('[') {
            section = line.trim_matches(['[', ']']).trim().to_string();
            // `[dependencies.foo]` names its alias in the header; a bare
            // restatement with no `package` key still needs recording.
            if let Some(alias) = section
                .rsplit_once('.')
                .filter(|(head, _)| dep_table(head).is_some() || head.ends_with("dependencies"))
                .map(|(_, alias)| alias.to_string())
            {
                record_dep(&mut out, &section, alias, "");
            }
            continue;
        }

        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let (key, value) = (unquote(key), value.trim());

        if section == "package" && key == "name" {
            out.package = Some(unquote(value).to_string());
            continue;
        }
        if dep_table(&section).is_none() {
            continue;
        }
        // A sub-table header already registered this alias; its body lines
        // only matter for a `package` key, handled by record_dep below.
        if section.contains('.')
            && dep_table(&section).is_some()
            && !section.ends_with("dependencies")
        {
            if key == "package" {
                record_dep_named(&mut out, &section, unquote(value).to_string());
            }
            continue;
        }

        let depth = depth_delta(value);
        if depth > 0 {
            pending = Some((key.to_string(), value.to_string(), depth));
        } else {
            record_dep(&mut out, &section, key.to_string(), value);
        }
    }
    out
}

/// Register `alias`, resolving its real package name from `body` if the inline
/// table renames it.
fn record_dep(out: &mut Manifest, section: &str, alias: String, body: &str) {
    if alias.is_empty() {
        return;
    }
    let package = PACKAGE_KEY
        .captures(body)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string())
        .unwrap_or_else(|| alias.clone());
    let target = match dep_table(section) {
        Some(DepTable::Workspace) => &mut out.workspace_deps,
        Some(DepTable::Local) => &mut out.deps,
        None => return,
    };
    target.insert(alias, package);
}

/// Point an already-registered `[dependencies.<alias>]` sub-table at the real
/// package named by its `package` key.
fn record_dep_named(out: &mut Manifest, section: &str, package: String) {
    let Some((_, alias)) = section.rsplit_once('.') else {
        return;
    };
    let target = match dep_table(section) {
        Some(DepTable::Workspace) => &mut out.workspace_deps,
        Some(DepTable::Local) => &mut out.deps,
        None => return,
    };
    target.insert(alias.to_string(), package);
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
            .build()
            .flatten()
        {
            if entry.file_name() != "Cargo.toml" {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(entry.path()) else {
                continue;
            };
            let manifest = parse_cargo_manifest(&text);
            workspace_deps.extend(manifest.workspace_deps.clone());

            let dir = entry
                .path()
                .parent()
                .and_then(|p| p.strip_prefix(root).ok())
                .and_then(|p| p.to_str())
                .map(|p| p.replace('\\', "/"))
                .unwrap_or_default();
            if let Some(name) = manifest.package.clone() {
                manifests.push(Unit {
                    lang: LANG_RUST,
                    name,
                    root: dir,
                    deps: manifest.deps,
                });
            }
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

    /// The innermost unit whose root contains `file_rel`.
    pub fn resolve(&self, file_rel: &str) -> Option<&Unit> {
        self.units.iter().find(|u| {
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
            return UnitContext::unnamed();
        };
        let (id, module_base) = target_of(unit, file_rel);
        let known: BTreeSet<&str> = self.units.iter().map(|u| u.name.as_str()).collect();

        // Workspace-level aliases first so a member's own declaration, which
        // is the more specific statement, overrides them.
        let mut siblings = BTreeMap::new();
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
        }
    }
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

/// The first path component of `rel`, without any `.rs` suffix — the target
/// name, whether the target is one file or a directory of them.
fn first_segment(rel: &str) -> &str {
    let head = rel.split('/').next().unwrap_or(rel);
    head.strip_suffix(".rs").unwrap_or(head)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    // ─── manifest parsing ───────────────────────────────────────────

    #[test]
    fn the_package_name_is_read_from_the_package_section() {
        let m = parse_cargo_manifest("[package]\nname = \"phronesis-mcp\"\n");
        assert_eq!(m.package.as_deref(), Some("phronesis-mcp"));
    }

    #[test]
    fn a_workspace_package_section_does_not_supply_the_package_name() {
        // `[workspace.package]` holds inherited *defaults*, not this crate's
        // identity. Reading it would name every member after the workspace.
        let m = parse_cargo_manifest("[workspace.package]\nname = \"nope\"\n");
        assert_eq!(m.package, None);
    }

    #[test]
    fn a_virtual_workspace_manifest_declares_no_package() {
        let m = parse_cargo_manifest("[workspace]\nresolver = \"2\"\nmembers = [\"crates/*\"]\n");
        assert_eq!(m.package, None);
    }

    #[test]
    fn a_simple_dependency_maps_its_alias_to_itself() {
        let m = parse_cargo_manifest("[dependencies]\nserde = \"1\"\n");
        assert_eq!(m.deps.get("serde").map(String::as_str), Some("serde"));
    }

    #[test]
    fn a_renamed_dependency_resolves_to_its_real_package() {
        // Without this, `use phr::…` finds no unit and the edge vanishes.
        let m = parse_cargo_manifest(
            "[dependencies]\nphr = { version = \"0.22\", path = \"../phronesis\", package = \"phronesis\" }\n",
        );
        assert_eq!(m.deps.get("phr").map(String::as_str), Some("phronesis"));
    }

    #[test]
    fn a_multi_line_inline_table_is_read_to_its_closing_brace() {
        let m = parse_cargo_manifest(
            "[dependencies]\nphr = {\n  version = \"0.22\",\n  package = \"phronesis\",\n}\n",
        );
        assert_eq!(m.deps.get("phr").map(String::as_str), Some("phronesis"));
    }

    #[test]
    fn a_key_merely_ending_in_package_is_not_a_rename() {
        let m = parse_cargo_manifest("[dependencies]\nfoo = { default-package = \"bar\" }\n");
        assert_eq!(m.deps.get("foo").map(String::as_str), Some("foo"));
    }

    #[test]
    fn a_dependency_sub_table_registers_its_alias() {
        let m = parse_cargo_manifest("[dependencies.serde]\nversion = \"1\"\n");
        assert_eq!(m.deps.get("serde").map(String::as_str), Some("serde"));
    }

    #[test]
    fn a_dependency_sub_table_honours_its_package_key() {
        let m = parse_cargo_manifest("[dependencies.phr]\npackage = \"phronesis\"\n");
        assert_eq!(m.deps.get("phr").map(String::as_str), Some("phronesis"));
    }

    #[test]
    fn dev_dependencies_count_as_dependencies() {
        let m = parse_cargo_manifest("[dev-dependencies]\ntempfile = \"3\"\n");
        assert!(m.deps.contains_key("tempfile"));
    }

    #[test]
    fn target_specific_dependencies_count_as_dependencies() {
        let m = parse_cargo_manifest("[target.'cfg(unix)'.dependencies]\nlibc = \"0.2\"\n");
        assert!(m.deps.contains_key("libc"));
    }

    #[test]
    fn workspace_dependencies_are_kept_separate_from_local_ones() {
        let m =
            parse_cargo_manifest("[workspace.dependencies]\nphr = { package = \"phronesis\" }\n");
        assert!(m.deps.is_empty());
        assert_eq!(
            m.workspace_deps.get("phr").map(String::as_str),
            Some("phronesis")
        );
    }

    #[test]
    fn a_comment_is_not_mistaken_for_a_value() {
        let m = parse_cargo_manifest("[package]\nname = \"real\" # not \"fake\"\n");
        assert_eq!(m.package.as_deref(), Some("real"));
    }

    #[test]
    fn a_hash_inside_a_string_does_not_start_a_comment() {
        let m = parse_cargo_manifest("[package]\nname = \"a#b\"\n");
        assert_eq!(m.package.as_deref(), Some("a#b"));
    }

    // ─── unit identity ──────────────────────────────────────────────

    fn unit(name: &str, root: &str, deps: &[(&str, &str)]) -> Unit {
        Unit {
            lang: LANG_RUST,
            name: name.into(),
            root: root.into(),
            deps: deps
                .iter()
                .map(|(a, p)| ((*a).to_string(), (*p).to_string()))
                .collect(),
        }
    }

    #[test]
    fn a_unit_id_carries_both_language_and_package() {
        assert_eq!(unit("phronesis", "", &[]).id(), "rust:phronesis");
    }

    #[test]
    fn a_file_resolves_to_the_unit_that_contains_it() {
        let m = UnitMap::from_units(vec![
            unit("phronesis", "crates/phronesis", &[]),
            unit("phronesis-mcp", "crates/phronesis-mcp", &[]),
        ]);
        assert_eq!(
            m.resolve("crates/phronesis-mcp/src/lib.rs").map(Unit::id),
            Some("rust:phronesis-mcp".to_string())
        );
    }

    #[test]
    fn the_innermost_unit_wins_for_a_nested_workspace() {
        let m = UnitMap::from_units(vec![
            unit("outer", "", &[]),
            unit("inner", "crates/inner", &[]),
        ]);
        assert_eq!(
            m.resolve("crates/inner/src/lib.rs").map(Unit::id),
            Some("rust:inner".to_string())
        );
    }

    #[test]
    fn a_root_named_as_a_prefix_of_another_does_not_capture_it() {
        // `crates/phronesis` must not claim `crates/phronesis-mcp/...`.
        let m = UnitMap::from_units(vec![
            unit("phronesis", "crates/phronesis", &[]),
            unit("phronesis-mcp", "crates/phronesis-mcp", &[]),
        ]);
        assert_eq!(
            m.resolve("crates/phronesis-mcp/src/lib.rs").map(Unit::id),
            Some("rust:phronesis-mcp".to_string())
        );
    }

    #[test]
    fn a_file_under_no_unit_gets_the_unnamed_context() {
        let m = UnitMap::from_units(vec![unit("inner", "crates/inner", &[])]);
        assert_eq!(m.context_for("other/src/a.rs"), UnitContext::unnamed());
    }

    #[test]
    fn a_packages_library_and_default_binary_are_distinct_units() {
        let m = UnitMap::from_units(vec![unit("app", "crates/app", &[])]);
        assert_eq!(
            m.context_for("crates/app/src/lib.rs").id,
            "rust:app".to_string()
        );
        assert_eq!(
            m.context_for("crates/app/src/main.rs").id,
            "rust:app#bin:app".to_string()
        );
    }

    #[test]
    fn a_targets_module_base_is_its_crate_root_file() {
        let m = UnitMap::from_units(vec![unit("app", "crates/app", &[])]);
        for (file, base) in [
            ("crates/app/src/lib.rs", "crates/app/src/lib"),
            ("crates/app/src/graph/store.rs", "crates/app/src/lib"),
            ("crates/app/src/main.rs", "crates/app/src/main"),
            ("crates/app/src/bin/tool.rs", "crates/app/src/bin/tool"),
            ("crates/app/benches/sync.rs", "crates/app/benches/sync"),
            ("crates/app/tests/hooks/helper.rs", "crates/app/tests/hooks"),
            ("crates/app/build.rs", "crates/app/build"),
        ] {
            assert_eq!(m.context_for(file).module_base, base, "{file}");
        }
    }

    #[test]
    fn a_root_package_states_its_module_base_without_a_leading_slash() {
        let m = UnitMap::from_units(vec![unit("app", "", &[])]);
        assert_eq!(m.context_for("src/lib.rs").module_base, "src/lib");
    }

    #[test]
    fn a_file_under_no_unit_has_no_module_base() {
        // Nothing is known to strip, so the extractor keeps its own
        // `src/`-anchored heuristic.
        let m = UnitMap::from_units(vec![unit("inner", "crates/inner", &[])]);
        assert!(m.context_for("other/a.rs").module_base.is_empty());
    }

    #[test]
    fn a_binary_can_reach_its_packages_library_by_extern_name() {
        let m = UnitMap::from_units(vec![unit("my-app", "crates/app", &[])]);
        let ctx = m.context_for("crates/app/src/main.rs");
        assert_eq!(
            ctx.siblings.get("my_app").map(String::as_str),
            Some("rust:my-app")
        );
    }

    #[test]
    fn an_empty_map_names_every_file_unnamed() {
        assert_eq!(
            UnitMap::default().context_for("src/a.rs").id,
            "rust:crate".to_string()
        );
    }

    // ─── sibling resolution ─────────────────────────────────────────

    #[test]
    fn a_dependency_on_a_sibling_unit_becomes_a_reachable_namespace() {
        let m = UnitMap::from_units(vec![
            unit("phronesis", "crates/phronesis", &[]),
            unit(
                "phronesis-mcp",
                "crates/phronesis-mcp",
                &[("phr", "phronesis")],
            ),
        ]);
        let ctx = m.context_for("crates/phronesis-mcp/src/lib.rs");
        assert_eq!(
            ctx.siblings.get("phr").map(String::as_str),
            Some("rust:phronesis")
        );
    }

    #[test]
    fn a_third_party_dependency_is_not_a_sibling() {
        // A node with no definitions hanging off it is worse than no node.
        let m = UnitMap::from_units(vec![unit("mine", "", &[("serde", "serde")])]);
        assert!(m.context_for("src/a.rs").siblings.is_empty());
    }

    #[test]
    fn a_unit_is_never_its_own_sibling() {
        let m = UnitMap::from_units(vec![unit("mine", "", &[("mine", "mine")])]);
        assert!(m.context_for("src/a.rs").siblings.is_empty());
    }

    // ─── discovery from disk ────────────────────────────────────────

    fn write(root: &Path, rel: &str, body: &str) {
        let p = root.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        std::fs::write(p, body).expect("write");
    }

    #[test]
    fn discovery_finds_every_member_of_a_workspace() {
        let d = TempDir::new().expect("tempdir");
        write(
            d.path(),
            "Cargo.toml",
            "[workspace]\nmembers = [\"crates/*\"]\n",
        );
        write(d.path(), "crates/a/Cargo.toml", "[package]\nname = \"a\"\n");
        write(d.path(), "crates/b/Cargo.toml", "[package]\nname = \"b\"\n");
        let m = UnitMap::discover(d.path());
        assert_eq!(
            m.resolve("crates/a/src/lib.rs").map(Unit::id).as_deref(),
            Some("rust:a")
        );
        assert_eq!(
            m.resolve("crates/b/src/lib.rs").map(Unit::id).as_deref(),
            Some("rust:b")
        );
    }

    #[test]
    fn two_crates_with_same_named_modules_get_distinct_namespaces() {
        // The bug this module exists to fix: without a unit prefix both files
        // claim `crate::wme` and merge into one node.
        let d = TempDir::new().expect("tempdir");
        write(d.path(), "crates/a/Cargo.toml", "[package]\nname = \"a\"\n");
        write(d.path(), "crates/b/Cargo.toml", "[package]\nname = \"b\"\n");
        let m = UnitMap::discover(d.path());
        assert_ne!(
            m.context_for("crates/a/src/wme.rs").id,
            m.context_for("crates/b/src/wme.rs").id
        );
    }

    #[test]
    fn a_workspace_level_alias_is_inherited_by_members() {
        let d = TempDir::new().expect("tempdir");
        write(
            d.path(),
            "Cargo.toml",
            "[workspace]\nmembers = [\"crates/*\"]\n[workspace.dependencies]\nphr = { package = \"core-lib\" }\n",
        );
        write(
            d.path(),
            "crates/app/Cargo.toml",
            "[package]\nname = \"app\"\n[dependencies]\nphr.workspace = true\n",
        );
        write(
            d.path(),
            "crates/core-lib/Cargo.toml",
            "[package]\nname = \"core-lib\"\n",
        );
        let ctx = UnitMap::discover(d.path()).context_for("crates/app/src/lib.rs");
        assert_eq!(
            ctx.siblings.get("phr").map(String::as_str),
            Some("rust:core-lib")
        );
    }

    #[test]
    fn a_virtual_root_manifest_defines_no_unit() {
        let d = TempDir::new().expect("tempdir");
        write(
            d.path(),
            "Cargo.toml",
            "[workspace]\nmembers = [\"crates/*\"]\n",
        );
        assert!(UnitMap::discover(d.path()).resolve("src/a.rs").is_none());
    }
}
