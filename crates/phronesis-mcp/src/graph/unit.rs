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

/// Language tag for the Python extractor.
pub const LANG_PYTHON: &str = "python";

/// Language tag for the TypeScript extractor.
pub const LANG_TYPESCRIPT: &str = "typescript";

/// The language that owns a source file, by extension. A file in no known
/// language belongs to no unit — better to name nothing than to name it under
/// whichever manifest happens to sit nearest.
pub fn lang_of_path(file_rel: &str) -> Option<&'static str> {
    match file_rel.rsplit_once('.') {
        Some((_, "rs")) => Some(LANG_RUST),
        Some((_, "py")) => Some(LANG_PYTHON),
        Some((_, "ts" | "tsx" | "mts" | "cts")) => Some(LANG_TYPESCRIPT),
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
        LANG_PYTHON | LANG_TYPESCRIPT => "project",
        _ => UNNAMED,
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

/// Parse the subset of `pyproject.toml` that bears on identity: the
/// distribution name, under PEP 621's `[project]` or Poetry's `[tool.poetry]`.
///
/// No dependency aliases. Python has no rename-on-import at the distribution
/// level — `import x` names the import package directly — so the alias map
/// that Cargo needs has no Python counterpart.
pub fn parse_pyproject_manifest(text: &str) -> Manifest {
    let mut out = Manifest::default();
    let mut section = String::new();
    for raw in text.lines() {
        let line = strip_comment(raw).trim();
        if line.starts_with('[') {
            section = line.trim_matches(['[', ']']).trim().to_string();
            continue;
        }
        if section != "project" && section != "tool.poetry" {
            continue;
        }
        if let Some((key, value)) = line.split_once('=')
            && unquote(key) == "name"
        {
            out.package = Some(unquote(value).to_string());
            // PEP 621 wins over Poetry when a file carries both.
            if section == "project" {
                return out;
            }
        }
    }
    out
}

/// Parse the one field of `package.json` that bears on identity: `name`.
///
/// Real JSON, not the hand-rolled scanner used for TOML: `package.json` is
/// JSON by definition and `serde_json` is already a dependency, so there is
/// no reason to approximate it. A file that does not parse declares no
/// package, which loses a unit rather than inventing one.
pub fn parse_package_json(text: &str) -> Manifest {
    let mut out = Manifest::default();
    if let Ok(serde_json::Value::Object(map)) = serde_json::from_str(text)
        && let Some(serde_json::Value::String(name)) = map.get("name")
        && !name.is_empty()
    {
        out.package = Some(name.clone());
    }
    out
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

/// Strip `//` line comments, `/* … */` block comments, and trailing commas
/// so `serde_json` can read a `tsconfig.json`.
///
/// TypeScript accepts JSONC here and real projects use it — `tsc --init`'s
/// own generated file opens with a `/* … */` banner — so refusing comments
/// would silently lose resolution rules on the most ordinary files.
/// Both comment forms, and the trailing-comma pass, track string state
/// (with backslash escapes) so nothing inside a string literal is mistaken
/// for comment or container syntax.
fn strip_jsonc(text: &str) -> String {
    #[derive(PartialEq)]
    enum State {
        Normal,
        InString,
        InLineComment,
        InBlockComment,
    }

    let mut out = String::with_capacity(text.len());
    let mut state = State::Normal;
    let mut escaped = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match state {
            State::InString => {
                out.push(c);
                if escaped {
                    escaped = false;
                } else if c == '\\' {
                    escaped = true;
                } else if c == '"' {
                    state = State::Normal;
                }
            }
            State::InLineComment => {
                if c == '\n' {
                    out.push('\n');
                    state = State::Normal;
                }
                // Everything else inside a line comment, including `/*` or
                // `"`, is just text — it does not open a nested state.
            }
            State::InBlockComment => {
                if c == '*' && chars.peek() == Some(&'/') {
                    chars.next();
                    state = State::Normal;
                }
                // A `//` or `"` inside a block comment is likewise inert;
                // only `*/` ends the comment.
            }
            State::Normal => match c {
                '"' => {
                    state = State::InString;
                    out.push(c);
                }
                '/' if chars.peek() == Some(&'/') => {
                    chars.next();
                    state = State::InLineComment;
                }
                '/' if chars.peek() == Some(&'*') => {
                    chars.next();
                    state = State::InBlockComment;
                }
                _ => out.push(c),
            },
        }
    }

    // Trailing commas: a comma whose next non-whitespace character closes a
    // container. String-aware (with escapes) so a `,}` inside a string
    // value is never mistaken for one.
    let chars: Vec<char> = out.chars().collect();
    let mut in_string = false;
    let mut escaped = false;
    let mut cleaned = String::with_capacity(out.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if in_string {
            cleaned.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            i += 1;
            continue;
        }
        if c == '"' {
            in_string = true;
            cleaned.push(c);
            i += 1;
            continue;
        }
        if c == ',' {
            let next = chars[i + 1..].iter().find(|n| !n.is_whitespace());
            if matches!(next, Some('}') | Some(']')) {
                i += 1;
                continue;
            }
        }
        cleaned.push(c);
        i += 1;
    }
    cleaned
}

/// Parse the resolution-relevant subset of a `tsconfig.json`.
///
/// `extends` is deliberately not followed here — see Task 4, which resolves
/// the chain on disk where the referenced files are readable.
pub fn parse_tsconfig(text: &str) -> TsConfig {
    let mut out = TsConfig::default();
    let Ok(serde_json::Value::Object(root)) = serde_json::from_str(&strip_jsonc(text)) else {
        return out;
    };
    let Some(serde_json::Value::Object(options)) = root.get("compilerOptions") else {
        return out;
    };
    if let Some(serde_json::Value::String(base)) = options.get("baseUrl") {
        out.base_url = base
            .trim_start_matches("./")
            .trim_end_matches('/')
            .trim_matches('.')
            .trim_matches('/')
            .to_string();
    }
    if let Some(serde_json::Value::Object(paths)) = options.get("paths") {
        for (alias, targets) in paths {
            let Some(serde_json::Value::Array(list)) = Some(targets) else {
                continue;
            };
            let targets: Vec<String> = list
                .iter()
                .filter_map(|t| t.as_str().map(|s| s.trim_start_matches("./").to_string()))
                .collect();
            if !targets.is_empty() {
                out.paths.insert(alias.clone(), targets);
            }
        }
    }
    out
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
                _ => continue,
            };
            let Ok(text) = std::fs::read_to_string(entry.path()) else {
                continue;
            };
            let manifest = match lang {
                LANG_RUST => parse_cargo_manifest(&text),
                LANG_PYTHON => parse_pyproject_manifest(&text),
                _ => parse_package_json(&text),
            };
            workspace_deps.extend(manifest.workspace_deps.clone());

            let dir = entry
                .path()
                .parent()
                .and_then(|p| p.strip_prefix(root).ok())
                .and_then(|p| p.to_str())
                .map(|p| p.replace('\\', "/"))
                .unwrap_or_default();
            if let Some(name) = manifest.package.clone() {
                // A `src/` directory beside the manifest means the src
                // layout, where imports are rooted one level deeper. Read
                // from disk rather than guessed: both layouts are current
                // practice and neither is declared in the manifest.
                let import_root =
                    if lang == LANG_PYTHON && entry.path().with_file_name("src").is_dir() {
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
                manifests.push(Unit {
                    lang,
                    name,
                    root: dir,
                    deps: manifest.deps,
                    import_root,
                    packages,
                    ts,
                    files,
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
            };
        }

        let mut siblings = BTreeMap::new();
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
    // Python has no compilation targets. Its distribution is the whole
    // namespace, a test module is an ordinary module, and imports are rooted
    // at the layout's import root. Giving it a `#test:` suffix by analogy
    // would split one namespace into several that can never join.
    if unit.lang == LANG_PYTHON {
        return (unit.id(), unit.import_root.clone());
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

/// Import packages a Python distribution provides, read from its import
/// root: a directory holding `__init__.py`, or a bare top-level module.
///
/// Read from disk rather than declared, because neither layout states it in
/// the manifest and the distribution name is frequently not the package name.
fn top_level_packages(import_root: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(import_root) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if path.is_dir() && path.join("__init__.py").is_file() {
            out.push(name.to_string());
        } else if let Some(stem) = name.strip_suffix(".py")
            && stem != "__init__"
        {
            out.push(stem.to_string());
        }
    }
    out.sort();
    out
}

/// Join a repo-relative directory to a child name, keeping the repo root
/// spelled as the empty string rather than a leading slash.
fn join_rel(dir: &str, child: &str) -> String {
    if dir.is_empty() {
        child.to_string()
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

/// Depth cap for `extends`. A chain longer than this is a configuration
/// mistake or a cycle; stopping is better than recursing forever.
const MAX_EXTENDS_DEPTH: usize = 8;

/// Read a `tsconfig.json` and everything it extends, child winning.
///
/// Only `extends` targets that are relative paths are followed. A bare
/// specifier resolves inside `node_modules`, which this graph never reads.
fn read_tsconfig_chain(path: &Path, depth: usize) -> TsConfig {
    if depth >= MAX_EXTENDS_DEPTH {
        return TsConfig::default();
    }
    let Ok(text) = std::fs::read_to_string(path) else {
        return TsConfig::default();
    };
    let own = parse_tsconfig(&text);

    let parent = serde_json::from_str::<serde_json::Value>(&strip_jsonc(&text))
        .ok()
        .and_then(|v| v.get("extends")?.as_str().map(str::to_string))
        .filter(|e| e.starts_with('.'))
        .and_then(|e| {
            let mut candidate = path.parent()?.join(&e);
            if candidate.extension().is_none() {
                candidate.set_extension("json");
            }
            Some(read_tsconfig_chain(&candidate, depth + 1))
        });

    let Some(mut merged) = parent else {
        return own;
    };
    if !own.base_url.is_empty() {
        merged.base_url = own.base_url;
    }
    merged.paths.extend(own.paths);
    merged
}

/// Repo-relative TypeScript sources under `unit_abs`, honouring `.gitignore`,
/// excluding `node_modules` unconditionally, and stopping at the boundary of
/// any nested unit.
///
/// A nested `package.json` starts a unit of its own; descending past it
/// would let the outer unit's file index claim files that actually belong to
/// the inner one. `resolve` is membership-based against this index (see its
/// doc comment), so a polluted index does not just misname a file — it can
/// resolve an import in the outer unit to a file that identifies itself
/// under the inner unit's `id`/`module_base`, producing an edge that never
/// joins the file's own `declares_module`. Better to under-index (parent
/// misses a file it never owned) than to over-index (parent claims a file it
/// doesn't own).
fn index_typescript_files(root: &Path, unit_abs: &Path, unit_rel: &str) -> Vec<String> {
    let mut out = Vec::new();
    let unit_abs_owned = unit_abs.to_path_buf();
    for entry in ignore::WalkBuilder::new(unit_abs)
        .hidden(true)
        .filter_entry(move |e| {
            if e.file_name() == "node_modules" {
                return false;
            }
            // The unit's own root always carries a package.json; only a
            // *nested* directory carrying one marks a boundary to stop at.
            e.path() == unit_abs_owned
                || !e
                    .file_type()
                    .is_some_and(|t| t.is_dir() && e.path().join("package.json").is_file())
        })
        .build()
        .flatten()
    {
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        if lang_of_path(entry.path().to_str().unwrap_or("")) != Some(LANG_TYPESCRIPT) {
            continue;
        }
        if let Ok(rel) = entry.path().strip_prefix(root)
            && let Some(rel) = rel.to_str()
        {
            out.push(rel.replace('\\', "/"));
        }
    }
    let _ = unit_rel;
    out.sort();
    out
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
            // Rust targets compute their own root; these fields are Python's.
            import_root: String::new(),
            packages: Vec::new(),
            ts: TsConfig::default(),
            files: Vec::new(),
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

#[cfg(test)]
mod python_tests {
    use super::*;
    use tempfile::TempDir;

    fn write(root: &Path, rel: &str, body: &str) {
        let p = root.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        std::fs::write(p, body).expect("write");
    }

    #[test]
    fn a_pyproject_declares_its_distribution_name() {
        let m = parse_pyproject_manifest("[project]\nname = \"py-side\"\nversion = \"0.1.0\"\n");
        assert_eq!(m.package.as_deref(), Some("py-side"));
    }

    #[test]
    fn a_poetry_pyproject_declares_its_distribution_name() {
        // Poetry predates PEP 621 and is still widespread.
        let m = parse_pyproject_manifest("[tool.poetry]\nname = \"legacy\"\n");
        assert_eq!(m.package.as_deref(), Some("legacy"));
    }

    #[test]
    fn a_pyproject_without_a_name_declares_no_distribution() {
        // A build-backend-only pyproject.toml is common and names nothing.
        let m = parse_pyproject_manifest("[build-system]\nrequires = [\"setuptools\"]\n");
        assert_eq!(m.package, None);
    }

    #[test]
    fn an_unclaimed_python_file_falls_back_to_a_python_namespace() {
        // The fallback must follow the file's language. Naming a stray .py
        // file `rust:crate::…` would merge it with Rust modules and make the
        // language tag a lie exactly where there is least evidence.
        let m = UnitMap::default();
        assert_eq!(m.context_for("scripts/tool.py").id, "python:project");
    }

    #[test]
    fn discovery_finds_a_python_distribution() {
        let d = TempDir::new().expect("tempdir");
        write(d.path(), "pyproject.toml", "[project]\nname = \"pyside\"\n");
        let m = UnitMap::discover(d.path());
        assert_eq!(
            m.resolve("src/pyside/utils.py").map(Unit::id).as_deref(),
            Some("python:pyside")
        );
    }

    #[test]
    fn a_sibling_distributions_packages_are_reachable_namespaces() {
        // Python has no distribution-level import aliasing: source writes the
        // *package* name, which need not match the distribution name, so the
        // sibling map has to be keyed by what is actually on disk.
        let d = TempDir::new().expect("tempdir");
        write(
            d.path(),
            "libs/core/pyproject.toml",
            "[project]\nname = \"core-lib\"\n",
        );
        write(d.path(), "libs/core/src/core/__init__.py", "");
        write(
            d.path(),
            "libs/app/pyproject.toml",
            "[project]\nname = \"app\"\n",
        );
        write(d.path(), "libs/app/src/app/__init__.py", "");

        let ctx = UnitMap::discover(d.path()).context_for("libs/app/src/app/__init__.py");
        assert_eq!(
            ctx.siblings.get("core").map(String::as_str),
            Some("python:core-lib"),
            "keyed by package name on disk, valued by distribution unit id"
        );
    }

    #[test]
    fn a_distribution_is_never_its_own_sibling() {
        let d = TempDir::new().expect("tempdir");
        write(d.path(), "pyproject.toml", "[project]\nname = \"solo\"\n");
        write(d.path(), "src/solo/__init__.py", "");
        let ctx = UnitMap::discover(d.path()).context_for("src/solo/mod.py");
        assert!(ctx.siblings.is_empty(), "{:?}", ctx.siblings);
    }

    #[test]
    fn a_python_file_never_resolves_to_a_cargo_package() {
        // Both manifests can sit in one repository, and the innermost-root
        // rule alone would hand a .py file whichever package is nearer.
        let d = TempDir::new().expect("tempdir");
        write(d.path(), "pyproject.toml", "[project]\nname = \"pyside\"\n");
        write(
            d.path(),
            "crates/inner/Cargo.toml",
            "[package]\nname = \"inner\"\n",
        );
        let m = UnitMap::discover(d.path());
        assert_eq!(
            m.resolve("crates/inner/helper.py").map(Unit::id).as_deref(),
            Some("python:pyside"),
            "a .py file under a Cargo package still belongs to the Python distribution"
        );
        assert_eq!(
            m.resolve("crates/inner/src/lib.rs")
                .map(Unit::id)
                .as_deref(),
            Some("rust:inner")
        );
    }
}

#[cfg(test)]
mod python_layout_tests {
    use super::*;
    use tempfile::TempDir;

    fn write(root: &Path, rel: &str, body: &str) {
        let p = root.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        std::fs::write(p, body).expect("write");
    }

    fn py_project(layout: &[&str]) -> TempDir {
        let d = TempDir::new().expect("tempdir");
        write(d.path(), "pyproject.toml", "[project]\nname = \"pyside\"\n");
        for rel in layout {
            write(d.path(), rel, "");
        }
        d
    }

    #[test]
    fn a_src_layout_roots_modules_at_the_src_directory() {
        // src/pyside/utils.py is the module `pyside.utils`, so `src` must be
        // stripped but `pyside` must not.
        let d = py_project(&["src/pyside/__init__.py"]);
        let m = UnitMap::discover(d.path());
        assert_eq!(m.context_for("src/pyside/utils.py").module_base, "src");
    }

    #[test]
    fn a_flat_layout_roots_modules_at_the_project_directory() {
        // Without a src/ directory the import root is the project root
        // itself: pyside/utils.py is `pyside.utils`.
        let d = py_project(&["pyside/__init__.py"]);
        let m = UnitMap::discover(d.path());
        assert!(m.context_for("pyside/utils.py").module_base.is_empty());
    }

    #[test]
    fn a_python_unit_has_no_compilation_target_suffix() {
        // Cargo's #bin:/#test: split exists because those compile as separate
        // crates. Python has no equivalent — a test module is just a module.
        let d = py_project(&["src/pyside/__init__.py"]);
        let m = UnitMap::discover(d.path());
        assert_eq!(m.context_for("src/pyside/utils.py").id, "python:pyside");
        assert_eq!(m.context_for("tests/test_utils.py").id, "python:pyside");
    }
}

#[cfg(test)]
mod typescript_tests {
    use super::*;

    #[test]
    fn typescript_extensions_map_to_the_typescript_language() {
        for path in ["a.ts", "a.tsx", "a.mts", "a.cts"] {
            assert_eq!(lang_of_path(path), Some(LANG_TYPESCRIPT), "{path}");
        }
    }

    #[test]
    fn an_unclaimed_typescript_file_falls_back_to_a_typescript_namespace() {
        // The fallback follows the file's own language, as Python's does.
        let m = UnitMap::default();
        assert_eq!(m.context_for("scripts/tool.ts").id, "typescript:project");
    }

    #[test]
    fn a_package_json_declares_its_package_name() {
        let m = parse_package_json(r#"{"name": "myapp", "version": "1.0.0"}"#);
        assert_eq!(m.package.as_deref(), Some("myapp"));
    }

    #[test]
    fn a_scoped_package_name_is_kept_whole() {
        // `@org/pkg` is one name, not a path. Splitting it would invent a unit.
        let m = parse_package_json(r#"{"name": "@yourorg/billing"}"#);
        assert_eq!(m.package.as_deref(), Some("@yourorg/billing"));
    }

    #[test]
    fn a_package_json_without_a_name_declares_no_package() {
        // Common in app roots that are not published.
        let m = parse_package_json(r#"{"private": true, "scripts": {}}"#);
        assert_eq!(m.package, None);
    }

    #[test]
    fn malformed_package_json_declares_no_package() {
        // Degrades to "no unit" rather than guessing a name.
        assert_eq!(parse_package_json("{not json").package, None);
    }

    #[test]
    fn a_tsconfig_without_compiler_options_yields_defaults() {
        assert_eq!(parse_tsconfig("{}"), TsConfig::default());
    }

    #[test]
    fn base_url_is_normalized_without_dot_slash_or_trailing_slash() {
        let c = parse_tsconfig(r#"{"compilerOptions": {"baseUrl": "./src/"}}"#);
        assert_eq!(c.base_url, "src");
    }

    #[test]
    fn base_url_of_dot_means_the_unit_root() {
        let c = parse_tsconfig(r#"{"compilerOptions": {"baseUrl": "."}}"#);
        assert_eq!(c.base_url, "");
    }

    #[test]
    fn paths_keep_their_wildcards_verbatim() {
        let c = parse_tsconfig(
            r#"{"compilerOptions": {"baseUrl": "src", "paths": {"@app/*": ["app/*"]}}}"#,
        );
        assert_eq!(c.paths.get("@app/*"), Some(&vec!["app/*".to_string()]));
    }

    #[test]
    fn a_path_alias_may_have_several_targets() {
        // TypeScript tries each in order; resolution must keep them all.
        let c = parse_tsconfig(r#"{"compilerOptions": {"paths": {"~/*": ["lib/*", "vendor/*"]}}}"#);
        assert_eq!(
            c.paths.get("~/*"),
            Some(&vec!["lib/*".to_string(), "vendor/*".to_string()])
        );
    }

    #[test]
    fn a_tsconfig_with_comments_still_parses() {
        // tsconfig.json is JSONC in practice; TypeScript itself allows comments.
        let c = parse_tsconfig(
            "{\n  // the source root\n  \"compilerOptions\": {\"baseUrl\": \"src\"}\n}",
        );
        assert_eq!(c.base_url, "src");
    }

    #[test]
    fn a_tsconfig_with_trailing_commas_still_parses() {
        let c = parse_tsconfig(r#"{"compilerOptions": {"baseUrl": "src",},}"#);
        assert_eq!(c.base_url, "src");
    }

    #[test]
    fn malformed_tsconfig_yields_defaults_rather_than_guesses() {
        assert_eq!(parse_tsconfig("{not json"), TsConfig::default());
    }

    #[test]
    fn a_double_slash_inside_a_string_is_not_treated_as_a_comment() {
        // A url like "https://example.com" must survive strip_jsonc intact;
        // treating the `//` as a comment would truncate the value mid-string
        // and break JSON parsing entirely (or worse, silently corrupt it).
        let c = parse_tsconfig(
            r#"{"compilerOptions": {"baseUrl": "src"}, "//comment": "https://example.com"}"#,
        );
        assert_eq!(c.base_url, "src");
    }

    #[test]
    fn the_tsc_init_banner_with_a_url_in_a_block_comment_still_parses() {
        // The literal header `tsc --init` writes. A `//` inside the block
        // comment must not be read as a line-comment start — that would eat
        // the closing `*/` and corrupt everything after it.
        let c = parse_tsconfig(
            "{\n  /* Visit https://aka.ms/tsconfig to read more about this file */\n  \"compilerOptions\": { \"baseUrl\": \"src\" }\n}",
        );
        assert_eq!(c.base_url, "src");
    }

    #[test]
    fn a_block_comment_on_its_own_line_is_stripped() {
        let c = parse_tsconfig(
            "{\n  /* explanatory */\n  \"compilerOptions\": {\"baseUrl\": \"src\"}\n}",
        );
        assert_eq!(c.base_url, "src");
    }

    #[test]
    fn a_slash_star_inside_a_string_is_not_treated_as_a_comment_start() {
        let c = parse_tsconfig(
            r#"{"compilerOptions": {"baseUrl": "src"}, "note": "see /* not a comment */ here"}"#,
        );
        assert_eq!(c.base_url, "src");
    }

    #[test]
    fn a_comma_and_brace_inside_a_string_path_target_survive_intact() {
        // The trailing-comma pass must be string-aware too: `,}` inside a
        // string value is data, not a container closer.
        let c = parse_tsconfig(r#"{"compilerOptions": {"paths": {"@x/*": ["a,}b/*"]}}}"#);
        assert_eq!(c.paths.get("@x/*"), Some(&vec!["a,}b/*".to_string()]));
    }

    // ─── discovery from disk ────────────────────────────────────────

    use tempfile::TempDir;

    fn write(root: &Path, rel: &str, body: &str) {
        let p = root.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        std::fs::write(p, body).expect("write");
    }

    #[test]
    fn discovery_finds_a_typescript_package() {
        let d = TempDir::new().expect("tempdir");
        write(d.path(), "package.json", r#"{"name": "myapp"}"#);
        write(d.path(), "src/index.ts", "");
        let m = UnitMap::discover(d.path());
        assert_eq!(
            m.resolve("src/index.ts").map(Unit::id).as_deref(),
            Some("typescript:myapp")
        );
    }

    #[test]
    fn two_independent_packages_in_one_tree_are_two_units() {
        // Not a monorepo — just two projects. The common case.
        let d = TempDir::new().expect("tempdir");
        write(d.path(), "frontend/package.json", r#"{"name": "web"}"#);
        write(d.path(), "frontend/src/a.ts", "");
        write(d.path(), "server/package.json", r#"{"name": "api"}"#);
        write(d.path(), "server/src/a.ts", "");
        let m = UnitMap::discover(d.path());
        assert_eq!(
            m.resolve("frontend/src/a.ts").map(Unit::id).as_deref(),
            Some("typescript:web")
        );
        assert_eq!(
            m.resolve("server/src/a.ts").map(Unit::id).as_deref(),
            Some("typescript:api")
        );
    }

    #[test]
    fn node_modules_never_defines_a_unit() {
        // Every dependency ships a package.json. Walking them would mint
        // hundreds of units and index tens of thousands of files.
        let d = TempDir::new().expect("tempdir");
        write(d.path(), "package.json", r#"{"name": "myapp"}"#);
        write(
            d.path(),
            "node_modules/left-pad/package.json",
            r#"{"name": "left-pad"}"#,
        );
        write(d.path(), "node_modules/left-pad/index.ts", "");
        let m = UnitMap::discover(d.path());
        assert_eq!(
            m.resolve("node_modules/left-pad/index.ts").map(Unit::id),
            None,
            "a dependency's file belongs to no unit of ours"
        );
    }

    #[test]
    fn a_units_file_index_excludes_node_modules() {
        let d = TempDir::new().expect("tempdir");
        write(d.path(), "package.json", r#"{"name": "myapp"}"#);
        write(d.path(), "src/a.ts", "");
        write(d.path(), "node_modules/dep/b.ts", "");
        let ctx = UnitMap::discover(d.path()).context_for("src/a.ts");
        assert!(
            ctx.files.contains(&"src/a.ts".to_string()),
            "{:?}",
            ctx.files
        );
        assert!(
            !ctx.files.iter().any(|f| f.contains("node_modules")),
            "{:?}",
            ctx.files
        );
    }

    #[test]
    fn a_units_tsconfig_is_read_into_its_context() {
        let d = TempDir::new().expect("tempdir");
        write(d.path(), "package.json", r#"{"name": "myapp"}"#);
        write(
            d.path(),
            "tsconfig.json",
            r#"{"compilerOptions": {"baseUrl": "src", "paths": {"@app/*": ["app/*"]}}}"#,
        );
        write(d.path(), "src/a.ts", "");
        let ctx = UnitMap::discover(d.path()).context_for("src/a.ts");
        assert_eq!(ctx.ts.base_url, "src");
        assert_eq!(ctx.ts.paths.get("@app/*"), Some(&vec!["app/*".to_string()]));
    }

    #[test]
    fn an_extends_chain_is_followed() {
        // Shared base configs are standard; not following `extends` loses the
        // aliases that most projects define exactly once, in the base.
        let d = TempDir::new().expect("tempdir");
        write(d.path(), "package.json", r#"{"name": "myapp"}"#);
        write(
            d.path(),
            "tsconfig.base.json",
            r#"{"compilerOptions": {"baseUrl": "src", "paths": {"@app/*": ["app/*"]}}}"#,
        );
        write(
            d.path(),
            "tsconfig.json",
            r#"{"extends": "./tsconfig.base.json"}"#,
        );
        write(d.path(), "src/a.ts", "");
        let ctx = UnitMap::discover(d.path()).context_for("src/a.ts");
        assert_eq!(ctx.ts.base_url, "src");
        assert_eq!(ctx.ts.paths.get("@app/*"), Some(&vec!["app/*".to_string()]));
    }

    #[test]
    fn a_child_tsconfig_overrides_what_it_extends() {
        let d = TempDir::new().expect("tempdir");
        write(d.path(), "package.json", r#"{"name": "myapp"}"#);
        write(
            d.path(),
            "base.json",
            r#"{"compilerOptions": {"baseUrl": "old"}}"#,
        );
        write(
            d.path(),
            "tsconfig.json",
            r#"{"extends": "./base.json", "compilerOptions": {"baseUrl": "src"}}"#,
        );
        write(d.path(), "src/a.ts", "");
        let ctx = UnitMap::discover(d.path()).context_for("src/a.ts");
        assert_eq!(ctx.ts.base_url, "src");
    }

    #[test]
    fn a_self_referential_extends_chain_terminates() {
        // A cycle here should not hang or overflow the stack — the depth cap
        // must actually stop recursion.
        let d = TempDir::new().expect("tempdir");
        write(d.path(), "package.json", r#"{"name": "myapp"}"#);
        write(
            d.path(),
            "tsconfig.json",
            r#"{"extends": "./tsconfig.json", "compilerOptions": {"baseUrl": "src"}}"#,
        );
        write(d.path(), "src/a.ts", "");
        let ctx = UnitMap::discover(d.path()).context_for("src/a.ts");
        // The file's own baseUrl still applies; the cycle just stops feeding
        // more "parent" data back in.
        assert_eq!(ctx.ts.base_url, "src");
    }

    #[test]
    fn a_nested_units_files_do_not_leak_into_the_outer_units_index() {
        // A unit's `files` must contain only files that resolve to that
        // unit. Without a boundary at the nested package.json, the outer
        // unit's index would claim the inner unit's sources too, and a later
        // import resolution against that index could name a file under the
        // wrong unit's id/module_base entirely.
        let d = TempDir::new().expect("tempdir");
        write(d.path(), "package.json", r#"{"name": "root"}"#);
        write(d.path(), "src/root.ts", "");
        write(
            d.path(),
            "packages/inner/package.json",
            r#"{"name": "inner"}"#,
        );
        write(d.path(), "packages/inner/src/a.ts", "");
        let m = UnitMap::discover(d.path());

        let outer = m.context_for("src/root.ts");
        assert_eq!(outer.id, "typescript:root");
        assert!(
            outer.files.contains(&"src/root.ts".to_string()),
            "{:?}",
            outer.files
        );
        assert!(
            !outer.files.contains(&"packages/inner/src/a.ts".to_string()),
            "outer unit's files leaked the inner unit's source: {:?}",
            outer.files
        );

        let inner = m.context_for("packages/inner/src/a.ts");
        assert_eq!(inner.id, "typescript:inner");
        assert!(
            inner.files.contains(&"packages/inner/src/a.ts".to_string()),
            "{:?}",
            inner.files
        );
    }

    #[test]
    fn a_file_created_after_discovery_is_invisible_until_rediscovered() {
        // `resolve` and `context_for` are membership-based against the
        // `files` index captured at `discover` time. Nothing in this crate
        // caches a `UnitMap` across saves today (sync always rediscovers
        // fresh before resolving), but the invariant is public API and must
        // be pinned: a stale map does not silently join the wrong unit, it
        // falls back to the unnamed context.
        let d = TempDir::new().expect("tempdir");
        write(d.path(), "package.json", r#"{"name": "root"}"#);
        write(d.path(), "src/a.ts", "");
        let m = UnitMap::discover(d.path());

        write(d.path(), "src/late.ts", "");

        assert_eq!(
            m.context_for("src/late.ts").id,
            "typescript:project",
            "a file created after discovery must not resolve into the unit \
             it would belong to on a fresh discovery"
        );
    }
}
