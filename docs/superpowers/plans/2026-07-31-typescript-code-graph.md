# TypeScript Code Graph Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Extract structural graph facts from TypeScript so the `structural` pack's two rules fire on TypeScript projects.

**Architecture:** A third extractor beside `graph/extract.rs` (Rust) and `graph/python.rs` (Python), sharing the edge vocabulary and `::` separator and nothing else. Import resolution is pre-computed in `UnitMap::discover` — which already owns disk access — and handed to the extractor via `UnitContext`, so extractors stay pure functions of `(path, content, unit)`.

**Tech Stack:** Rust 2024, tree-sitter 0.25 with `tree-sitter-typescript` 0.23.2 (already a dependency), `serde_json` for `package.json` / `tsconfig.json`.

**Spec:** `docs/superpowers/specs/2026-07-31-typescript-code-graph-design.md`

## Global Constraints

- Segments join with `::` in every language — never `.` or `/`. See `.phronesis/wiki/decisions/2026-07-27-graph-identity-separator.md`.
- Extractors are pure: `fn(file_path: &str, content: &str, unit: &UnitContext) -> Extracted`. No filesystem access inside an extractor.
- A parse failure returns `Extracted::unparseable()` — never an empty edge set. An empty extraction erases the file's evidence and reports the graph fresh.
- An unresolved *relative* import increments `skipped`. Never drop it silently.
- `node_modules` is excluded unconditionally, not via `.gitignore`.
- Rules ship `warn`-only. No `block` promotion in this plan.
- Every task ends green on: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`.
- Run `cargo test --workspace` **unpiped**. Piping or redirecting to `/dev/null` loses the confidence signal and blocks the commit — see `.phronesis/wiki/decisions/2026-07-06-piped-test-output-loses-signal.md`.

---

## File Structure

| File | Responsibility |
|---|---|
| `crates/phronesis-mcp/src/graph/unit.rs` (modify) | Add `LANG_TYPESCRIPT`, `.ts`/`.tsx`/`.mts`/`.cts` in `lang_of_path`, `ts:project` fallback, `package.json` + `tsconfig.json` parsing, file index, `node_modules` exclusion |
| `crates/phronesis-mcp/src/graph/resolve.rs` (create) | Specifier → module identity resolution. Pure, unit-tested in isolation |
| `crates/phronesis-mcp/src/graph/typescript.rs` (create) | The extractor: `extract_typescript` |
| `crates/phronesis-mcp/src/graph/sync.rs` (modify) | Dispatch `.ts`/`.tsx`/`.mts`/`.cts`, add to `TRACKED_EXTENSIONS` |
| `crates/phronesis-mcp/src/graph/mod.rs` (modify) | `pub mod resolve; pub mod typescript;` |
| `crates/phronesis-mcp/src/init.rs` (modify) | Two TypeScript rules in the structural pack |
| `crates/phronesis-mcp/tests/graph_structural_rules.rs` (modify) | Integration: TS graph, cycle, three-language coexistence |

Resolution lives in its own file rather than inside `typescript.rs` because it is the risky part, it is pure, and it deserves to be tested without a parser in the loop. `unit.rs` is already ~1,100 lines; this plan adds to it rather than splitting it, because a split is orthogonal refactoring — it is noted as follow-up in the spec, not done here.

---

### Task 1: Language tag and extension mapping

**Files:**
- Modify: `crates/phronesis-mcp/src/graph/unit.rs`

**Interfaces:**
- Produces: `pub const LANG_TYPESCRIPT: &str = "typescript";` — `lang_of_path` returns it for `.ts`, `.tsx`, `.mts`, `.cts`; `unnamed_name` returns `"project"` for it.

- [ ] **Step 1: Write the failing tests**

Add to the `python_tests` module's sibling — create a new `mod typescript_tests` at the end of `unit.rs`:

```rust
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
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p phronesis-mcp --lib graph::unit::typescript_tests`
Expected: FAIL — `cannot find value LANG_TYPESCRIPT in this scope`

- [ ] **Step 3: Implement**

In `crates/phronesis-mcp/src/graph/unit.rs`, after the `LANG_PYTHON` const:

```rust
/// Language tag for the TypeScript extractor.
pub const LANG_TYPESCRIPT: &str = "typescript";
```

Extend `lang_of_path`:

```rust
pub fn lang_of_path(file_rel: &str) -> Option<&'static str> {
    match file_rel.rsplit_once('.') {
        Some((_, "rs")) => Some(LANG_RUST),
        Some((_, "py")) => Some(LANG_PYTHON),
        Some((_, "ts" | "tsx" | "mts" | "cts")) => Some(LANG_TYPESCRIPT),
        _ => None,
    }
}
```

Extend `unnamed_name`:

```rust
fn unnamed_name(lang: &str) -> &'static str {
    match lang {
        LANG_PYTHON | LANG_TYPESCRIPT => "project",
        _ => UNNAMED,
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p phronesis-mcp --lib graph::unit::typescript_tests`
Expected: PASS, 2 tests

- [ ] **Step 5: Commit**

```bash
git add crates/phronesis-mcp/src/graph/unit.rs
git commit -m "feat(graph): recognize TypeScript source extensions"
```

---

### Task 2: Parse `package.json` for unit identity

**Files:**
- Modify: `crates/phronesis-mcp/src/graph/unit.rs`

**Interfaces:**
- Produces: `pub fn parse_package_json(text: &str) -> Manifest` — sets `Manifest::package` from the `name` field, leaves `deps` and `workspace_deps` empty.

- [ ] **Step 1: Write the failing tests**

Add to `mod typescript_tests`:

```rust
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
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p phronesis-mcp --lib graph::unit::typescript_tests`
Expected: FAIL — `cannot find function parse_package_json`

- [ ] **Step 3: Implement**

Add to `unit.rs`, beside `parse_pyproject_manifest`:

```rust
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
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p phronesis-mcp --lib graph::unit::typescript_tests`
Expected: PASS, 6 tests

- [ ] **Step 5: Commit**

```bash
git add crates/phronesis-mcp/src/graph/unit.rs
git commit -m "feat(graph): read npm package identity from package.json"
```

---

### Task 3: Parse `tsconfig.json` for `baseUrl` and `paths`

**Files:**
- Modify: `crates/phronesis-mcp/src/graph/unit.rs`

**Interfaces:**
- Produces:
```rust
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct TsConfig {
    /// `compilerOptions.baseUrl`, normalized relative to the unit root with
    /// no leading `./` and no trailing `/`. Empty when unset.
    pub base_url: String,
    /// `compilerOptions.paths`: alias pattern -> replacement patterns, both
    /// keeping their `*` wildcard verbatim.
    pub paths: BTreeMap<String, Vec<String>>,
}

pub fn parse_tsconfig(text: &str) -> TsConfig;
```

- [ ] **Step 1: Write the failing tests**

Add to `mod typescript_tests`:

```rust
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
    let c = parse_tsconfig(
        r#"{"compilerOptions": {"paths": {"~/*": ["lib/*", "vendor/*"]}}}"#,
    );
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
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p phronesis-mcp --lib graph::unit::typescript_tests`
Expected: FAIL — `cannot find type TsConfig`

- [ ] **Step 3: Implement**

Add to `unit.rs`:

```rust
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

/// Strip `//` line comments and trailing commas so `serde_json` can read a
/// `tsconfig.json`.
///
/// TypeScript accepts JSONC here and real projects use it, so refusing
/// comments would silently lose resolution rules on the most ordinary files.
/// `//` inside a string is left alone.
fn strip_jsonc(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_string = false;
    let mut escaped = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if in_string {
            out.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        match c {
            '"' => {
                in_string = true;
                out.push(c);
            }
            '/' if chars.peek() == Some(&'/') => {
                for skipped in chars.by_ref() {
                    if skipped == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            _ => out.push(c),
        }
    }
    // Trailing commas: a comma whose next non-whitespace character closes a
    // container.
    let bytes: Vec<char> = out.chars().collect();
    let mut cleaned = String::with_capacity(out.len());
    for (i, c) in bytes.iter().enumerate() {
        if *c == ',' {
            let next = bytes[i + 1..].iter().find(|n| !n.is_whitespace());
            if matches!(next, Some('}') | Some(']')) {
                continue;
            }
        }
        cleaned.push(*c);
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
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p phronesis-mcp --lib graph::unit::typescript_tests`
Expected: PASS, 14 tests

- [ ] **Step 5: Commit**

```bash
git add crates/phronesis-mcp/src/graph/unit.rs
git commit -m "feat(graph): parse tsconfig baseUrl and path aliases"
```

---

### Task 4: Discover TypeScript units, excluding `node_modules`

**Files:**
- Modify: `crates/phronesis-mcp/src/graph/unit.rs`

**Interfaces:**
- Consumes: `parse_package_json` (Task 2), `parse_tsconfig` (Task 3).
- Produces: `Unit` gains `pub ts: TsConfig` and `pub files: Vec<String>` (repo-relative paths of this unit's TypeScript sources, sorted). `UnitContext` gains the same two fields. Every existing `Unit { .. }` literal in tests must add `ts: TsConfig::default(), files: Vec::new()`.

- [ ] **Step 1: Write the failing tests**

Add to `mod typescript_tests`:

```rust
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
    write(d.path(), "node_modules/left-pad/package.json", r#"{"name": "left-pad"}"#);
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
    assert!(ctx.files.contains(&"src/a.ts".to_string()), "{:?}", ctx.files);
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
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p phronesis-mcp --lib graph::unit`
Expected: FAIL — `no field ts on type UnitContext`

- [ ] **Step 3: Implement**

Add the two fields to `Unit`:

```rust
    /// Resolution rules for this unit. TypeScript only; empty elsewhere.
    pub ts: TsConfig,
    /// Repo-relative TypeScript sources belonging to this unit, sorted.
    /// Resolution is a lookup against this rather than disk I/O, which keeps
    /// the extractor a pure function of its inputs.
    pub files: Vec<String>,
```

Add the same two to `UnitContext`, and to `UnitContext::unnamed_for`:

```rust
    pub fn unnamed_for(lang: &str) -> Self {
        UnitContext {
            id: format!("{lang}:{}", unnamed_name(lang)),
            module_base: String::new(),
            siblings: BTreeMap::new(),
            ts: TsConfig::default(),
            files: Vec::new(),
        }
    }
```

In `UnitMap::discover`, extend the manifest match and skip `node_modules`:

```rust
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
```

and select the parser:

```rust
            let manifest = match lang {
                LANG_RUST => parse_cargo_manifest(&text),
                LANG_PYTHON => parse_pyproject_manifest(&text),
                _ => parse_package_json(&text),
            };
```

After computing `dir`, for TypeScript units read the config chain and index files:

```rust
                let (ts, files) = if lang == LANG_TYPESCRIPT {
                    let dir_abs = root.join(&dir);
                    (
                        read_tsconfig_chain(&dir_abs.join("tsconfig.json"), 0),
                        index_typescript_files(root, &dir_abs, &dir),
                    )
                } else {
                    (TsConfig::default(), Vec::new())
                };
```

and pass `ts` / `files` into the `Unit` literal. Then add the two helpers:

```rust
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

/// Repo-relative TypeScript sources under `unit_abs`, honouring `.gitignore`
/// and excluding `node_modules` unconditionally.
fn index_typescript_files(root: &Path, unit_abs: &Path, unit_rel: &str) -> Vec<String> {
    let mut out = Vec::new();
    for entry in ignore::WalkBuilder::new(unit_abs)
        .hidden(true)
        .filter_entry(|e| e.file_name() != "node_modules")
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
```

In `context_for`, carry both fields through for TypeScript units — insert before the Rust sibling logic, mirroring the Python early return:

```rust
        if unit.lang == LANG_TYPESCRIPT {
            return UnitContext {
                id,
                module_base: join_rel(&unit.root, &unit.ts.base_url),
                siblings: BTreeMap::new(),
                ts: unit.ts.clone(),
                files: unit.files.clone(),
            };
        }
```

Finally, add `ts: TsConfig::default(), files: Vec::new()` to the `unit()` test helper in `mod tests`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p phronesis-mcp --lib graph::unit`
Expected: PASS — all existing unit tests plus 21 in `typescript_tests`

- [ ] **Step 5: Commit**

```bash
git add crates/phronesis-mcp/src/graph/unit.rs
git commit -m "feat(graph): discover TypeScript units with their tsconfig and file index"
```

---

### Task 5: Specifier resolution

**Files:**
- Create: `crates/phronesis-mcp/src/graph/resolve.rs`
- Modify: `crates/phronesis-mcp/src/graph/mod.rs`

**Interfaces:**
- Consumes: `UnitContext` with `ts` and `files` (Task 4).
- Produces:
```rust
/// Outcome of resolving one import specifier.
pub enum Resolution {
    /// Resolved to a file in this unit; carries its repo-relative path.
    File(String),
    /// Third-party. No edge, not an error.
    External,
    /// Names something in this project that could not be found.
    Unresolved,
}

pub fn resolve_specifier(
    specifier: &str,
    importing_file: &str,
    unit: &UnitContext,
) -> Resolution;

/// Module identity for a repo-relative TypeScript file, e.g.
/// `typescript:myapp::billing::charge`.
pub fn module_path(file_rel: &str, unit: &UnitContext) -> String;
```

- [ ] **Step 1: Write the failing tests**

Create `crates/phronesis-mcp/src/graph/resolve.rs` containing only the test module for now:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::unit::TsConfig;
    use std::collections::BTreeMap;

    fn ctx(files: &[&str], base_url: &str, paths: &[(&str, &[&str])]) -> UnitContext {
        UnitContext {
            id: "typescript:myapp".to_string(),
            module_base: if base_url.is_empty() {
                String::new()
            } else {
                base_url.to_string()
            },
            siblings: BTreeMap::new(),
            ts: TsConfig {
                base_url: base_url.to_string(),
                paths: paths
                    .iter()
                    .map(|(k, v)| {
                        ((*k).to_string(), v.iter().map(|s| (*s).to_string()).collect())
                    })
                    .collect(),
            },
            files: files.iter().map(|f| (*f).to_string()).collect(),
        }
    }

    // ─── module identity ────────────────────────────────────────────

    #[test]
    fn a_file_maps_to_its_module_path() {
        let c = ctx(&["src/billing/charge.ts"], "src", &[]);
        assert_eq!(
            module_path("src/billing/charge.ts", &c),
            "typescript:myapp::billing::charge"
        );
    }

    #[test]
    fn an_index_file_names_its_directory() {
        let c = ctx(&["src/billing/index.ts"], "src", &[]);
        assert_eq!(
            module_path("src/billing/index.ts", &c),
            "typescript:myapp::billing"
        );
    }

    #[test]
    fn without_a_base_url_the_unit_root_is_the_module_root() {
        let c = ctx(&["lib/util.ts"], "", &[]);
        assert_eq!(module_path("lib/util.ts", &c), "typescript:myapp::lib::util");
    }

    #[test]
    fn a_tsx_extension_is_stripped_like_any_other() {
        let c = ctx(&["src/Button.tsx"], "src", &[]);
        assert_eq!(module_path("src/Button.tsx", &c), "typescript:myapp::Button");
    }

    // ─── relative specifiers ────────────────────────────────────────

    #[test]
    fn a_relative_specifier_resolves_to_a_sibling_file() {
        let c = ctx(&["src/a.ts", "src/billing.ts"], "src", &[]);
        assert_eq!(
            resolve_specifier("./billing", "src/a.ts", &c),
            Resolution::File("src/billing.ts".to_string())
        );
    }

    #[test]
    fn a_relative_specifier_resolves_to_a_directory_index() {
        let c = ctx(&["src/a.ts", "src/billing/index.ts"], "src", &[]);
        assert_eq!(
            resolve_specifier("./billing", "src/a.ts", &c),
            Resolution::File("src/billing/index.ts".to_string())
        );
    }

    #[test]
    fn a_parent_relative_specifier_climbs_a_directory() {
        let c = ctx(&["src/db/models.ts", "src/util.ts"], "src", &[]);
        assert_eq!(
            resolve_specifier("../util", "src/db/models.ts", &c),
            Resolution::File("src/util.ts".to_string())
        );
    }

    #[test]
    fn an_explicit_extension_resolves() {
        // `./x.js` is the ESM convention for a TypeScript `./x.ts`.
        let c = ctx(&["src/a.ts", "src/billing.ts"], "src", &[]);
        assert_eq!(
            resolve_specifier("./billing.js", "src/a.ts", &c),
            Resolution::File("src/billing.ts".to_string())
        );
    }

    #[test]
    fn a_file_wins_over_a_directory_of_the_same_name() {
        // TypeScript probes `billing.ts` before `billing/index.ts`.
        let c = ctx(
            &["src/a.ts", "src/billing.ts", "src/billing/index.ts"],
            "src",
            &[],
        );
        assert_eq!(
            resolve_specifier("./billing", "src/a.ts", &c),
            Resolution::File("src/billing.ts".to_string())
        );
    }

    #[test]
    fn an_unresolvable_relative_specifier_is_unresolved_not_external() {
        // A specifier starting with `.` names something in this project, so
        // failing to find it is our bug and must stay visible.
        let c = ctx(&["src/a.ts"], "src", &[]);
        assert_eq!(
            resolve_specifier("./missing", "src/a.ts", &c),
            Resolution::Unresolved
        );
    }

    // ─── aliases and baseUrl ────────────────────────────────────────

    #[test]
    fn a_path_alias_resolves() {
        let c = ctx(
            &["src/a.ts", "src/app/billing.ts"],
            "src",
            &[("@app/*", &["app/*"])],
        );
        assert_eq!(
            resolve_specifier("@app/billing", "src/a.ts", &c),
            Resolution::File("src/app/billing.ts".to_string())
        );
    }

    #[test]
    fn an_alias_with_several_targets_tries_each_in_order() {
        let c = ctx(
            &["src/a.ts", "src/vendor/x.ts"],
            "src",
            &[("~/*", &["lib/*", "vendor/*"])],
        );
        assert_eq!(
            resolve_specifier("~/x", "src/a.ts", &c),
            Resolution::File("src/vendor/x.ts".to_string())
        );
    }

    #[test]
    fn a_bare_specifier_resolves_against_base_url() {
        let c = ctx(&["src/a.ts", "src/util.ts"], "src", &[]);
        assert_eq!(
            resolve_specifier("util", "src/a.ts", &c),
            Resolution::File("src/util.ts".to_string())
        );
    }

    #[test]
    fn a_third_party_specifier_is_external() {
        let c = ctx(&["src/a.ts"], "src", &[]);
        assert_eq!(resolve_specifier("react", "src/a.ts", &c), Resolution::External);
    }

    #[test]
    fn a_scoped_third_party_specifier_is_external() {
        let c = ctx(&["src/a.ts"], "src", &[]);
        assert_eq!(
            resolve_specifier("@yourorg/shared", "src/a.ts", &c),
            Resolution::External
        );
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Add `pub mod resolve;` to `crates/phronesis-mcp/src/graph/mod.rs` (alphabetically, after `pub mod query;`).

Run: `cargo test -p phronesis-mcp --lib graph::resolve`
Expected: FAIL — `cannot find type Resolution in this scope`

- [ ] **Step 3: Implement**

Prepend to `resolve.rs`:

```rust
//! Turning a TypeScript import specifier into a module identity.
//!
//! Rust and Python imports name modules, so an edge falls out of the source
//! text. TypeScript imports name *paths*, which must be resolved against the
//! project's files and its `tsconfig.json` before any edge exists. This
//! module is that resolution, kept separate from the extractor because it is
//! the risky part and deserves testing without a parser in the loop.
//!
//! A missing import edge is invisible — it looks exactly like a codebase with
//! no such dependency — and `imports` feeds `in_cycle`, so a dropped edge is
//! a cycle silently unreported. Hence `Resolution::Unresolved` is a distinct
//! outcome from `External`: the first is our bug and gets counted, the second
//! is third-party and is correct to ignore.

use super::unit::UnitContext;

/// Extensions probed for a specifier without one, in TypeScript's order.
const PROBE_EXTENSIONS: &[&str] = &[".ts", ".tsx", ".d.ts", ".js", ".jsx", ".mts", ".cts"];

/// Extensions a specifier may carry that stand in for a TypeScript source.
const REWRITABLE_EXTENSIONS: &[&str] = &[".js", ".jsx", ".mjs", ".cjs"];

/// Outcome of resolving one import specifier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    /// Resolved to a file in this unit, as a repo-relative path.
    File(String),
    /// Third-party: not in this project, and correct to ignore.
    External,
    /// Names something in this project that could not be found. Counted, so
    /// a broken resolver cannot masquerade as a clean codebase.
    Unresolved,
}

/// Module identity for a repo-relative TypeScript file.
///
/// A pure function of the path, because resolution computes an identity from
/// two directions — the importer's specifier and the target's own path — and
/// an edge forms only when they agree.
pub fn module_path(file_rel: &str, unit: &UnitContext) -> String {
    let rel = strip_module_base(file_rel, &unit.module_base);
    let trimmed = strip_known_extension(rel);
    let mut segments: Vec<&str> = trimmed.split('/').filter(|s| !s.is_empty()).collect();
    if segments.last() == Some(&"index") {
        segments.pop();
    }
    std::iter::once(unit.id.as_str())
        .chain(segments)
        .collect::<Vec<_>>()
        .join("::")
}

fn strip_module_base<'a>(file_rel: &'a str, module_base: &str) -> &'a str {
    if module_base.is_empty() {
        return file_rel;
    }
    file_rel
        .strip_prefix(module_base)
        .and_then(|rest| rest.strip_prefix('/'))
        .unwrap_or(file_rel)
}

fn strip_known_extension(path: &str) -> &str {
    for ext in [".d.ts", ".tsx", ".ts", ".mts", ".cts", ".jsx", ".js"] {
        if let Some(stem) = path.strip_suffix(ext) {
            return stem;
        }
    }
    path
}

/// Resolve one import specifier from `importing_file`.
pub fn resolve_specifier(
    specifier: &str,
    importing_file: &str,
    unit: &UnitContext,
) -> Resolution {
    if specifier.starts_with('.') {
        let dir = importing_file.rsplit_once('/').map_or("", |(d, _)| d);
        let joined = normalize(&format!("{dir}/{specifier}"));
        return match probe(&joined, unit) {
            Some(found) => Resolution::File(found),
            // Relative means "inside this project" by definition.
            None => Resolution::Unresolved,
        };
    }

    for (alias, targets) in &unit.ts.paths {
        let Some(prefix) = alias.strip_suffix('*') else {
            if alias == specifier {
                for target in targets {
                    if let Some(found) = probe(&with_base(target, unit), unit) {
                        return Resolution::File(found);
                    }
                }
            }
            continue;
        };
        let Some(rest) = specifier.strip_prefix(prefix) else {
            continue;
        };
        for target in targets {
            let candidate = with_base(&target.replace('*', rest), unit);
            if let Some(found) = probe(&candidate, unit) {
                return Resolution::File(found);
            }
        }
    }

    if !unit.ts.base_url.is_empty() || !unit.module_base.is_empty() {
        if let Some(found) = probe(&with_base(specifier, unit), unit) {
            return Resolution::File(found);
        }
    }

    Resolution::External
}

/// Join a unit-relative path onto the module base (`baseUrl`).
fn with_base(path: &str, unit: &UnitContext) -> String {
    if unit.module_base.is_empty() {
        path.to_string()
    } else {
        format!("{}/{path}", unit.module_base)
    }
}

/// Find the indexed file a candidate path names, probing extensions and
/// `index` files in TypeScript's order.
fn probe(candidate: &str, unit: &UnitContext) -> Option<String> {
    let has = |p: &str| unit.files.iter().find(|f| f.as_str() == p).cloned();

    if let Some(found) = has(candidate) {
        return Some(found);
    }
    // `./x.js` is the ESM spelling of `./x.ts`.
    for ext in REWRITABLE_EXTENSIONS {
        if let Some(stem) = candidate.strip_suffix(ext) {
            for probe_ext in PROBE_EXTENSIONS {
                if let Some(found) = has(&format!("{stem}{probe_ext}")) {
                    return Some(found);
                }
            }
        }
    }
    for ext in PROBE_EXTENSIONS {
        if let Some(found) = has(&format!("{candidate}{ext}")) {
            return Some(found);
        }
    }
    for ext in PROBE_EXTENSIONS {
        if let Some(found) = has(&format!("{candidate}/index{ext}")) {
            return Some(found);
        }
    }
    None
}

/// Collapse `.` and `..` segments in a path.
fn normalize(path: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for segment in path.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            s => out.push(s),
        }
    }
    out.join("/")
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p phronesis-mcp --lib graph::resolve`
Expected: PASS, 15 tests

- [ ] **Step 5: Commit**

```bash
git add crates/phronesis-mcp/src/graph/resolve.rs crates/phronesis-mcp/src/graph/mod.rs
git commit -m "feat(graph): resolve TypeScript import specifiers to module identities"
```

---

### Task 6: The extractor — file_type, declares_module, defines_fn

**Files:**
- Create: `crates/phronesis-mcp/src/graph/typescript.rs`
- Modify: `crates/phronesis-mcp/src/graph/mod.rs`

**Interfaces:**
- Consumes: `resolve::module_path` (Task 5), `Extracted` / `Extracted::unparseable()` from `super::extract`.
- Produces: `pub fn extract_typescript(file_path: &str, content: &str, unit: &UnitContext) -> Extracted`.

- [ ] **Step 1: Write the failing tests**

Create `crates/phronesis-mcp/src/graph/typescript.rs` with the module doc, a `file_type` fn, and this test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::unit::TsConfig;
    use std::collections::BTreeMap;

    fn ctx(files: &[&str]) -> UnitContext {
        UnitContext {
            id: "typescript:myapp".to_string(),
            module_base: "src".to_string(),
            siblings: BTreeMap::new(),
            ts: TsConfig {
                base_url: "src".to_string(),
                paths: BTreeMap::new(),
            },
            files: files.iter().map(|f| (*f).to_string()).collect(),
        }
    }

    fn edges_of(x: &Extracted, p: &str) -> Vec<Vec<String>> {
        x.edges
            .iter()
            .filter(|e| e.p == p)
            .map(|e| e.a.clone())
            .collect()
    }

    // ─── file_type ──────────────────────────────────────────────────

    #[test]
    fn a_plain_module_is_production() {
        assert_eq!(file_type("src/billing.ts"), "production");
    }

    #[test]
    fn a_dot_test_file_is_a_test() {
        assert_eq!(file_type("src/billing.test.ts"), "test");
        assert_eq!(file_type("src/billing.spec.tsx"), "test");
    }

    #[test]
    fn a_file_under_a_tests_directory_is_a_test() {
        assert_eq!(file_type("src/__tests__/billing.ts"), "test");
    }

    // ─── declares_module ────────────────────────────────────────────

    #[test]
    fn a_file_declares_its_own_module() {
        let out = extract_typescript("src/billing.ts", "export const x = 1\n", &ctx(&["src/billing.ts"]));
        assert_eq!(
            edges_of(&out, "declares_module"),
            vec![vec![
                "src/billing.ts".to_string(),
                "typescript:myapp::billing".to_string()
            ]]
        );
    }

    // ─── defines_fn ─────────────────────────────────────────────────

    #[test]
    fn a_function_declaration_is_a_defined_function() {
        let out = extract_typescript(
            "src/billing.ts",
            "export function charge() { return 1 }\n",
            &ctx(&["src/billing.ts"]),
        );
        assert_eq!(
            edges_of(&out, "defines_fn")[0][1],
            "typescript:myapp::billing::charge"
        );
    }

    #[test]
    fn an_arrow_function_assigned_to_a_const_is_a_defined_function() {
        // The dominant style in modern TypeScript; missing it would leave
        // most codebases looking empty.
        let out = extract_typescript(
            "src/billing.ts",
            "export const charge = () => 1\n",
            &ctx(&["src/billing.ts"]),
        );
        assert_eq!(
            edges_of(&out, "defines_fn")[0][1],
            "typescript:myapp::billing::charge"
        );
    }

    #[test]
    fn a_class_method_is_qualified_by_its_class() {
        let out = extract_typescript(
            "src/billing.ts",
            "export class Ledger { charge() { return 1 } }\n",
            &ctx(&["src/billing.ts"]),
        );
        assert_eq!(
            edges_of(&out, "defines_fn")[0][1],
            "typescript:myapp::billing::Ledger::charge"
        );
    }

    #[test]
    fn a_non_function_const_is_not_a_defined_function() {
        let out = extract_typescript(
            "src/billing.ts",
            "export const RATE = 0.2\n",
            &ctx(&["src/billing.ts"]),
        );
        assert!(edges_of(&out, "defines_fn").is_empty());
    }

    // ─── guards ─────────────────────────────────────────────────────

    #[test]
    fn a_non_typescript_file_yields_nothing() {
        assert_eq!(
            extract_typescript("src/a.rs", "fn f() {}", &ctx(&[])),
            Extracted::default()
        );
    }

    #[test]
    fn unparseable_source_preserves_existing_evidence() {
        // Not an empty extraction: that would erase the file's edges and
        // report the graph fresh.
        let out = extract_typescript("src/billing.ts", "function ((( {", &ctx(&["src/billing.ts"]));
        assert!(out.parse_failed, "must signal parse failure");
        assert!(out.edges.is_empty());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Add `pub mod typescript;` to `crates/phronesis-mcp/src/graph/mod.rs`.

Run: `cargo test -p phronesis-mcp --lib graph::typescript`
Expected: FAIL — `cannot find function extract_typescript`

- [ ] **Step 3: Implement**

Prepend to `typescript.rs`:

```rust
//! The TypeScript sensor: structural edges from one `.ts` / `.tsx` file.
//!
//! Separate from `super::extract` and `super::python` because almost nothing
//! is shared. TypeScript has no declared module tree — the directory layout
//! is the module tree — and its imports name paths rather than modules, so
//! `super::resolve` does work neither other extractor needs.

use super::extract::Extracted;
use super::model::Edge;
use super::resolve::{self, Resolution};
use super::unit::UnitContext;
use crate::syntax::parsed::ParsedFile;
use std::collections::BTreeSet;
use tree_sitter::Node;

/// Classification of a TypeScript file, as the `file_type` relation.
///
/// Follows the conventions jest, vitest and mocha share: `*.test.*`,
/// `*.spec.*`, or anything beneath a `__tests__` directory.
fn file_type(file_path: &str) -> &'static str {
    let name = file_path.rsplit('/').next().unwrap_or(file_path);
    let stem = name.rsplit_once('.').map_or(name, |(s, _)| s);
    if stem.ends_with(".test") || stem.ends_with(".spec") {
        return "test";
    }
    if file_path.starts_with("__tests__/") || file_path.contains("/__tests__/") {
        return "test";
    }
    "production"
}

fn text<'a>(node: Node, source: &'a [u8]) -> &'a str {
    node.utf8_text(source).unwrap_or("")
}

/// True when a node is a function of any TypeScript spelling.
fn is_function_node(kind: &str) -> bool {
    matches!(
        kind,
        "function_expression" | "arrow_function" | "function_declaration" | "generator_function"
    )
}

struct Sensor<'a> {
    file_path: &'a str,
    source: &'a [u8],
    unit: &'a UnitContext,
    self_module: String,
    out: BTreeSet<(String, Vec<String>)>,
    skipped: usize,
}

impl Sensor<'_> {
    fn emit(&mut self, p: &str, args: &[&str]) {
        self.out
            .insert((p.to_string(), args.iter().map(|s| s.to_string()).collect()));
    }

    fn qualify(&self, scope: &[String], name: &str) -> String {
        let mut parts = vec![self.self_module.clone()];
        parts.extend(scope.iter().cloned());
        parts.push(name.to_string());
        parts.join("::")
    }

    fn walk(&mut self, node: Node, scope: &[String]) {
        match node.kind() {
            "function_declaration" | "generator_function_declaration" => {
                if let Some(name) = node.child_by_field_name("name") {
                    let qualified = self.qualify(scope, text(name, self.source));
                    let file = self.file_path.to_string();
                    self.emit("defines_fn", &[&file, &qualified]);
                } else {
                    self.skipped += 1;
                }
                return;
            }
            "class_declaration" => {
                let Some(name) = node.child_by_field_name("name") else {
                    self.skipped += 1;
                    return;
                };
                let mut inner = scope.to_vec();
                inner.push(text(name, self.source).to_string());
                if let Some(body) = node.child_by_field_name("body") {
                    self.walk_children(body, &inner);
                }
                return;
            }
            "method_definition" => {
                if let Some(name) = node.child_by_field_name("name") {
                    let qualified = self.qualify(scope, text(name, self.source));
                    let file = self.file_path.to_string();
                    self.emit("defines_fn", &[&file, &qualified]);
                } else {
                    self.skipped += 1;
                }
                return;
            }
            "variable_declarator" => {
                // `const charge = () => …` is the dominant modern spelling;
                // treating it as a plain binding would leave most codebases
                // looking as though they define nothing.
                let name = node.child_by_field_name("name");
                let value = node.child_by_field_name("value");
                if let (Some(name), Some(value)) = (name, value)
                    && is_function_node(value.kind())
                {
                    let qualified = self.qualify(scope, text(name, self.source));
                    let file = self.file_path.to_string();
                    self.emit("defines_fn", &[&file, &qualified]);
                    return;
                }
            }
            "import_statement" => {
                self.import(node);
                return;
            }
            _ => {}
        }
        self.walk_children(node, scope);
    }

    fn walk_children(&mut self, node: Node, scope: &[String]) {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor).collect::<Vec<_>>() {
            self.walk(child, scope);
        }
    }

    /// Resolve an `import … from "…"` to a module edge.
    fn import(&mut self, node: Node) {
        let Some(source_node) = node.child_by_field_name("source") else {
            return;
        };
        let specifier = text(source_node, self.source).trim_matches(['"', '\'', '`']);
        match resolve::resolve_specifier(specifier, self.file_path, self.unit) {
            Resolution::File(target_file) => {
                let target = resolve::module_path(&target_file, self.unit);
                if target != self.self_module {
                    let from = self.self_module.clone();
                    self.emit("imports", &[&from, &target]);
                }
            }
            // Third-party: a node with no definitions hanging off it is worse
            // than no node.
            Resolution::External => {}
            // Names something in this project that we could not find. Counted
            // so a broken resolver cannot look like a clean codebase.
            Resolution::Unresolved => self.skipped += 1,
        }
    }
}

/// Extract every base relation from one TypeScript file.
pub fn extract_typescript(
    file_path: &str,
    content: &str,
    unit: &UnitContext,
) -> Extracted {
    let Some(lang) = crate::graph::unit::lang_of_path(file_path) else {
        return Extracted::default();
    };
    if lang != crate::graph::unit::LANG_TYPESCRIPT {
        return Extracted::default();
    }

    let tsx = file_path.ends_with(".tsx");
    let Some(parsed) = ParsedFile::parse_typescript(content, tsx) else {
        return Extracted::unparseable();
    };
    let ParsedFile::TypeScript { tree, source } = &parsed else {
        return Extracted::unparseable();
    };
    if tree.root_node().has_error() {
        return Extracted::unparseable();
    }

    let self_module = resolve::module_path(file_path, unit);
    let mut sensor = Sensor {
        file_path,
        source: source.as_bytes(),
        unit,
        self_module: self_module.clone(),
        out: BTreeSet::new(),
        skipped: 0,
    };
    sensor.emit("file_type", &[file_path, file_type(file_path)]);
    sensor.emit("declares_module", &[file_path, &self_module]);
    sensor.walk(tree.root_node(), &[]);

    let edges = sensor
        .out
        .iter()
        .map(|(p, a)| Edge {
            p: p.clone(),
            a: a.clone(),
            src: file_path.to_string(),
            d: false,
        })
        .collect();
    Extracted {
        edges,
        skipped: sensor.skipped,
        parse_failed: false,
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p phronesis-mcp --lib graph::typescript`
Expected: PASS, 10 tests

- [ ] **Step 5: Commit**

```bash
git add crates/phronesis-mcp/src/graph/typescript.rs crates/phronesis-mcp/src/graph/mod.rs
git commit -m "feat(graph): extract TypeScript modules, functions and imports"
```

---

### Task 7: `tested_by` from test titles, and `calls_api` from `!`

**Files:**
- Modify: `crates/phronesis-mcp/src/graph/typescript.rs`

**Interfaces:**
- Consumes: the `Sensor` from Task 6.
- Produces: `tested_by` edges keyed on test title strings, `calls_api(fn, "non_null_assertion")` edges.

- [ ] **Step 1: Write the failing tests**

Add to `typescript.rs`'s `mod tests`:

```rust
// ─── tested_by ──────────────────────────────────────────────────

#[test]
fn a_test_callback_records_what_it_calls() {
    // TS tests are callbacks, not named functions, so the coverage source is
    // identified by its title string — the only stable identity available.
    let out = extract_typescript(
        "src/billing.test.ts",
        "it('charges the order', () => { charge(cart) })\n",
        &ctx(&["src/billing.test.ts"]),
    );
    assert_eq!(
        edges_of(&out, "tested_by"),
        vec![vec![
            "charge".to_string(),
            "typescript:myapp::billing.test::charges the order".to_string()
        ]]
    );
}

#[test]
fn a_test_spelled_with_test_rather_than_it_also_counts() {
    let out = extract_typescript(
        "src/billing.test.ts",
        "test('charges', () => { charge() })\n",
        &ctx(&["src/billing.test.ts"]),
    );
    assert_eq!(edges_of(&out, "tested_by")[0][0], "charge");
}

#[test]
fn a_helper_outside_a_test_callback_is_not_coverage() {
    // A helper's calls are not evidence that anything was verified — the
    // same rule Python applies to non-`test_*` functions.
    let out = extract_typescript(
        "src/billing.test.ts",
        "function buildFixture() { charge() }\n",
        &ctx(&["src/billing.test.ts"]),
    );
    assert!(edges_of(&out, "tested_by").is_empty());
}

#[test]
fn a_method_call_inside_a_test_records_the_method_name() {
    let out = extract_typescript(
        "src/billing.test.ts",
        "it('works', () => { ledger.charge() })\n",
        &ctx(&["src/billing.test.ts"]),
    );
    assert_eq!(edges_of(&out, "tested_by")[0][0], "charge");
}

// ─── calls_api ──────────────────────────────────────────────────

#[test]
fn a_non_null_assertion_is_a_watched_api_call() {
    let out = extract_typescript(
        "src/billing.ts",
        "export function charge(o?: Order) { return o!.total }\n",
        &ctx(&["src/billing.ts"]),
    );
    assert_eq!(
        edges_of(&out, "calls_api"),
        vec![vec![
            "typescript:myapp::billing::charge".to_string(),
            "non_null_assertion".to_string()
        ]]
    );
}

#[test]
fn a_function_without_assertions_calls_no_watched_api() {
    let out = extract_typescript(
        "src/billing.ts",
        "export function charge(o: Order) { return o.total }\n",
        &ctx(&["src/billing.ts"]),
    );
    assert!(edges_of(&out, "calls_api").is_empty());
}

#[test]
fn several_assertions_in_one_function_yield_one_edge() {
    let out = extract_typescript(
        "src/billing.ts",
        "export function charge(o?: Order) { return o!.total + o!.tax }\n",
        &ctx(&["src/billing.ts"]),
    );
    assert_eq!(edges_of(&out, "calls_api").len(), 1);
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p phronesis-mcp --lib graph::typescript`
Expected: FAIL — `tested_by` and `calls_api` edge lists are empty

- [ ] **Step 3: Implement**

Add to `Sensor`, and call `self.test_call(node)` from `walk`'s `_ => {}` arm before `walk_children` when `node.kind() == "call_expression"`:

```rust
    /// The title of a `it("…", …)` / `test("…", …)` call, if this is one.
    fn test_title(&self, node: Node) -> Option<String> {
        if node.kind() != "call_expression" {
            return None;
        }
        let function = node.child_by_field_name("function")?;
        if !matches!(text(function, self.source), "it" | "test") {
            return None;
        }
        let args = node.child_by_field_name("arguments")?;
        let mut cursor = args.walk();
        let first = args.children(&mut cursor).find(|c| {
            matches!(c.kind(), "string" | "template_string")
        })?;
        Some(
            text(first, self.source)
                .trim_matches(['"', '\'', '`'])
                .to_string(),
        )
    }

    /// Bare names of functions invoked in a body. Resolved by short name in
    /// `super::derive::untested`, exactly as Rust and Python do.
    fn called_names(&self, body: Node) -> BTreeSet<String> {
        let mut found = BTreeSet::new();
        let mut stack = vec![body];
        while let Some(n) = stack.pop() {
            if n.kind() == "call_expression"
                && let Some(f) = n.child_by_field_name("function")
            {
                let name = match f.kind() {
                    "identifier" => text(f, self.source).to_string(),
                    "member_expression" => f
                        .child_by_field_name("property")
                        .map(|p| text(p, self.source).to_string())
                        .unwrap_or_default(),
                    _ => String::new(),
                };
                if !name.is_empty() && !matches!(name.as_str(), "it" | "test" | "describe") {
                    found.insert(name);
                }
            }
            let mut cursor = n.walk();
            stack.extend(n.children(&mut cursor));
        }
        found
    }

    /// True when this subtree contains a non-null assertion.
    fn asserts_non_null(&self, node: Node) -> bool {
        let mut stack = vec![node];
        while let Some(n) = stack.pop() {
            if n.kind() == "non_null_expression" {
                return true;
            }
            let mut cursor = n.walk();
            stack.extend(n.children(&mut cursor));
        }
        false
    }
```

In `walk`, for the three definition arms (`function_declaration`,
`method_definition`, and the `variable_declarator` function case), after
emitting `defines_fn`, add the watchlist check — using the node's body:

```rust
                    if self.asserts_non_null(node) {
                        self.emit("calls_api", &[&qualified, "non_null_assertion"]);
                    }
```

And in `walk`'s default arm, before recursing:

```rust
            _ => {
                if let Some(title) = self.test_title(node) {
                    let qualified = format!("{}::{title}", self.self_module);
                    for callee in self.called_names(node) {
                        self.emit("tested_by", &[&callee, &qualified]);
                    }
                    return;
                }
            }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p phronesis-mcp --lib graph::typescript`
Expected: PASS, 17 tests

- [ ] **Step 5: Commit**

```bash
git add crates/phronesis-mcp/src/graph/typescript.rs
git commit -m "feat(graph): record TypeScript test coverage and non-null assertions"
```

---

### Task 8: Wire TypeScript into the save pipeline

**Files:**
- Modify: `crates/phronesis-mcp/src/graph/sync.rs`

**Interfaces:**
- Consumes: `extract_typescript` (Task 6).
- Produces: `TRACKED_EXTENSIONS` includes `.ts`, `.tsx`, `.mts`, `.cts`; `extract_one` dispatches to the TypeScript extractor.

- [ ] **Step 1: Write the failing test**

Add to `sync.rs`'s `mod tests`:

```rust
#[test]
fn saving_a_typescript_file_writes_its_base_edges() {
    let d = project();
    write(d.path(), "package.json", r#"{"name": "myapp"}"#);
    write(d.path(), "src/billing.ts", "export function charge() { return 1 }\n");
    rebuild(d.path()).expect("opt the project into the graph");
    let names: Vec<String> = edges(d.path())
        .into_iter()
        .filter(|e| e.p == "defines_fn")
        .filter_map(|e| e.a.get(1).cloned())
        .collect();
    assert!(
        names.contains(&"typescript:myapp::src::billing::charge".to_string())
            || names.contains(&"typescript:myapp::billing::charge".to_string()),
        "{names:?}"
    );
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p phronesis-mcp --lib graph::sync::tests::saving_a_typescript_file`
Expected: FAIL — no `defines_fn` edge, because `.ts` is not tracked

- [ ] **Step 3: Implement**

In `sync.rs`, extend the constant:

```rust
pub const TRACKED_EXTENSIONS: &[&str] = &[".rs", ".py", ".ts", ".tsx", ".mts", ".cts"];
```

Extend `tracked_files`'s extension filter:

```rust
        if !matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("rs") | Some("py") | Some("ts") | Some("tsx") | Some("mts") | Some("cts")
        ) {
            continue;
        }
```

Extend `extract_one`:

```rust
fn extract_one(rel: &str, content: &str, units: &UnitMap) -> super::extract::Extracted {
    let unit = units.context_for(rel);
    match super::unit::lang_of_path(rel) {
        Some(super::unit::LANG_PYTHON) => super::python::extract_python(rel, content, &unit),
        Some(super::unit::LANG_TYPESCRIPT) => {
            super::typescript::extract_typescript(rel, content, &unit)
        }
        _ => extract_rust(rel, content, DEFAULT_WATCHLIST, &unit),
    }
}
```

Also exclude `node_modules` from `tracked_files`'s walk, mirroring discovery:

```rust
    for entry in ignore::WalkBuilder::new(root)
        .hidden(true)
        .filter_entry(|e| e.file_name() != "node_modules")
        .build()
        .flatten()
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p phronesis-mcp --lib graph::`
Expected: PASS — all graph tests

- [ ] **Step 5: Commit**

```bash
git add crates/phronesis-mcp/src/graph/sync.rs
git commit -m "feat(graph): track and extract TypeScript files on save"
```

---

### Task 9: Ship the two TypeScript rules

**Files:**
- Modify: `crates/phronesis-mcp/src/init.rs`
- Modify: `docs/catalogue.html` (regenerated, not hand-edited)

**Interfaces:**
- Consumes: the `calls_api` / `in_cycle` edges from Tasks 6–8.
- Produces: rules `warn-ts-untested-risky-call` and `warn-ts-import-cycle` in `structural_rules()`.

- [ ] **Step 1: Write the failing test**

Add to `init.rs`'s test module, beside the existing structural-rule tests:

```rust
#[test]
fn structural_rules_cover_typescript() {
    let rules = structural_rules();
    let ids: Vec<String> = rules["rules"]
        .as_array()
        .expect("rules array")
        .iter()
        .filter_map(|r| r["id"].as_str().map(str::to_string))
        .collect();
    assert!(
        ids.contains(&"warn-ts-untested-risky-call".to_string()),
        "{ids:?}"
    );
    assert!(ids.contains(&"warn-ts-import-cycle".to_string()), "{ids:?}");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p phronesis-mcp --lib init::tests::structural_rules_cover_typescript`
Expected: FAIL — ids do not contain the TypeScript rules

- [ ] **Step 3: Implement**

In `structural_rules()`, append two rules to the array. Both `warn`, both
`audit: true`, both opening on `edited_file`, mirroring the Rust pair:

```rust
            {
                "id": "warn-ts-untested-risky-call",
                "phase": "pre",
                "priority": 20,
                "audit": true,
                "when": [
                    {"edited_file": "?file"},
                    {"file_type": ["?file", "production"]},
                    {"defines_fn": ["?file", "?func"]},
                    {"calls_api": ["?func", "non_null_assertion"]},
                    {"untested": ["?func"]}
                ],
                "then": {"warn": "`?func` uses a non-null assertion (`!`) and has no direct test. `!` tells the compiler a value cannot be null and produces no runtime check, so when the assumption is wrong the failure surfaces later and elsewhere. Add a test that exercises the null case, or narrow the type so the assertion is unnecessary."}
            },
            {
                "id": "warn-ts-import-cycle",
                "phase": "pre",
                "priority": 20,
                "audit": true,
                "when": [
                    {"edited_file": "?file"},
                    {"declares_module": ["?file", "?module"]},
                    {"in_cycle": ["?module", "?cycle"]}
                ],
                "then": {"warn": "Module `?module` is part of import cycle `?cycle`. Mutually importing modules can't be understood, tested, or bundled independently, and the cycle tends to attract more coupling over time. Move the shared items into a third module both can depend on."}
            }
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p phronesis-mcp --lib init`
Expected: PASS — including the existing `structural_rules_opt_into_audit` test, which now covers four rules

- [ ] **Step 5: Regenerate the catalogue and commit**

The catalogue is a generated artifact and drifts otherwise.

```bash
cargo build -p phronesis-mcp
./target/debug/phr-mcp catalogue
git add crates/phronesis-mcp/src/init.rs docs/catalogue.html
git commit -m "feat(init): ship TypeScript structural rules"
```

---

### Task 10: End-to-end through the real binary

**Files:**
- Modify: `crates/phronesis-mcp/tests/graph_structural_rules.rs`

**Interfaces:**
- Consumes: everything above, exercised through the `phr-mcp` binary rather than library calls.

- [ ] **Step 1: Write the failing tests**

Append to `tests/graph_structural_rules.rs`:

```rust
/// A TypeScript project with a two-module import cycle.
fn typescript_project() -> TempDir {
    let d = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(d.path().join("src")).expect("mkdir src");
    std::fs::write(d.path().join("package.json"), r#"{"name": "myapp"}"#).expect("pkg");
    std::fs::write(
        d.path().join("tsconfig.json"),
        r#"{"compilerOptions": {"baseUrl": "src"}}"#,
    )
    .expect("tsconfig");
    std::fs::write(
        d.path().join("src/orders.ts"),
        "import { charge } from './billing'\nexport function place() { return charge() }\n",
    )
    .expect("orders");
    std::fs::write(
        d.path().join("src/billing.ts"),
        "import { place } from './orders'\nexport function charge() { return place }\n",
    )
    .expect("billing");
    rebuild_graph(d.path());
    d
}

#[test]
fn a_typescript_project_produces_a_graph() {
    let d = typescript_project();
    let edges = store::load(&store::graph_path(d.path())).expect("load graph");
    let functions: Vec<String> = edges
        .iter()
        .filter(|e| e.p == "defines_fn")
        .filter_map(|e| e.a.get(1).cloned())
        .collect();
    assert!(
        functions.contains(&"typescript:myapp::orders::place".to_string()),
        "{functions:?}"
    );
}

#[test]
fn a_typescript_import_cycle_is_detected() {
    // Resolution is the whole feature: without it these two imports produce
    // no edges and the cycle is invisible.
    let d = typescript_project();
    let edges = store::load(&store::graph_path(d.path())).expect("load graph");
    let cycles: Vec<String> = edges
        .iter()
        .filter(|e| e.p == "in_cycle")
        .filter_map(|e| e.a.first().cloned())
        .collect();
    assert!(
        cycles.contains(&"typescript:myapp::orders".to_string()),
        "{cycles:?}"
    );
    assert!(
        cycles.contains(&"typescript:myapp::billing".to_string()),
        "{cycles:?}"
    );
}

#[test]
fn node_modules_never_enters_the_graph() {
    let d = typescript_project();
    std::fs::create_dir_all(d.path().join("node_modules/left-pad")).expect("mkdir");
    std::fs::write(
        d.path().join("node_modules/left-pad/package.json"),
        r#"{"name": "left-pad"}"#,
    )
    .expect("dep pkg");
    std::fs::write(
        d.path().join("node_modules/left-pad/index.ts"),
        "export function pad() { return 1 }\n",
    )
    .expect("dep src");
    rebuild_graph(d.path());
    let edges = store::load(&store::graph_path(d.path())).expect("load graph");
    assert!(
        !edges.iter().any(|e| e.src.contains("node_modules")),
        "a dependency's code must never be graphed"
    );
}

#[test]
fn three_languages_coexist_in_one_graph() {
    let d = TempDir::new().expect("tempdir");

    std::fs::create_dir_all(d.path().join("rs/src")).expect("mkdir rs");
    std::fs::write(
        d.path().join("rs/Cargo.toml"),
        "[package]\nname = \"rs-side\"\nversion = \"0.1.0\"\n",
    )
    .expect("cargo");
    std::fs::write(d.path().join("rs/src/lib.rs"), "pub fn load() -> u32 { 1 }\n").expect("rs");

    std::fs::create_dir_all(d.path().join("py/src/pyside")).expect("mkdir py");
    std::fs::write(d.path().join("py/pyproject.toml"), "[project]\nname = \"py-side\"\n")
        .expect("pyproject");
    std::fs::write(d.path().join("py/src/pyside/__init__.py"), "").expect("py init");
    std::fs::write(d.path().join("py/src/pyside/utils.py"), "def load():\n    return 1\n")
        .expect("py");

    std::fs::create_dir_all(d.path().join("ts/src")).expect("mkdir ts");
    std::fs::write(d.path().join("ts/package.json"), r#"{"name": "ts-side"}"#).expect("pkg");
    std::fs::write(
        d.path().join("ts/tsconfig.json"),
        r#"{"compilerOptions": {"baseUrl": "src"}}"#,
    )
    .expect("tsconfig");
    std::fs::write(d.path().join("ts/src/utils.ts"), "export function load() { return 1 }\n")
        .expect("ts");

    rebuild_graph(d.path());
    let edges = store::load(&store::graph_path(d.path())).expect("load graph");
    let functions: Vec<String> = edges
        .iter()
        .filter(|e| e.p == "defines_fn")
        .filter_map(|e| e.a.get(1).cloned())
        .collect();

    assert!(functions.iter().any(|f| f.starts_with("rust:")), "{functions:?}");
    assert!(functions.iter().any(|f| f.starts_with("python:")), "{functions:?}");
    assert!(
        functions.iter().any(|f| f.starts_with("typescript:")),
        "{functions:?}"
    );
    assert_eq!(
        functions.iter().filter(|f| f.ends_with("::load")).count(),
        3,
        "same-named functions in three languages must stay distinct: {functions:?}"
    );
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p phronesis-mcp --test graph_structural_rules typescript`
Expected: FAIL on `a_typescript_project_produces_a_graph` before Tasks 6–8 land; after them, run to confirm all four pass.

- [ ] **Step 3: No new implementation**

These tests exercise code written in Tasks 1–9. If any fails, the defect is in those tasks — fix it there rather than weakening the test.

- [ ] **Step 4: Run the full gate**

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Run `cargo test --workspace` unpiped. Expected: all pass.

- [ ] **Step 5: Commit**

```bash
git add crates/phronesis-mcp/tests/graph_structural_rules.rs
git commit -m "test(graph): end-to-end TypeScript extraction through the real binary"
```

---

### Task 11: Validate against a real TypeScript project

**Files:**
- Modify: `docs/specs/SPEC-triple-store-rete.md` (record the measurement)
- Modify: `crates/phronesis-mcp/CLAUDE.md` (pack description)
- Modify: `CHANGELOG.md`

**Interfaces:**
- Consumes: the complete extractor.

**This task is a merge gate, not a formality.** phronesis contains no TypeScript, so every test above runs on fixtures this plan also wrote. Synthetic fixtures pass while resolution quietly drops edges on real code — that is the failure mode this design exists to prevent, and it has already shipped twice in this codebase.

- [ ] **Step 1: Build and run against a real project**

```bash
cargo build -p phronesis-mcp
cd /path/to/a/real/typescript/project
/path/to/phronesis/target/debug/phr-mcp graph rebuild --path .
```

Record the reported counts: base edges, derived edges, **and `skipped`**.

- [ ] **Step 2: Check resolution actually resolved**

```bash
/path/to/phr-mcp graph query imports --path . | head -30
/path/to/phr-mcp graph query defines_fn --path . | wc -l
```

Confirm by inspection:
- `imports` edges exist and point at real modules, not a handful of stragglers.
- The `defines_fn` count is plausible for the codebase's size.
- `skipped` is small. **A large `skipped` means resolution is failing** — investigate before proceeding rather than shipping it.

- [ ] **Step 3: Check a known cycle, or confirm none**

```bash
/path/to/phr-mcp graph query in_cycle --path .
```

Compare against what the team knows. Zero cycles in a large codebase is a plausible answer *and* the signature of broken resolution — distinguishing the two is the point of Step 2.

- [ ] **Step 4: Record the measurement in the spec**

Add a subsection to `docs/specs/SPEC-triple-store-rete.md` §4.2 giving the
project's size, the edge counts, the `skipped` count, and the rebuild time.
Write what was measured, not what was hoped for.

- [ ] **Step 5: Update the pack description and changelog**

In `crates/phronesis-mcp/CLAUDE.md`, extend the `structural` pack bullet to
state TypeScript's coverage accurately — that it carries both rules, using
`!` as the watchlist, and how that claim differs from Rust's.

Add a `## [0.24.0]` entry to `CHANGELOG.md` covering the extractor, the two
rules, and the stated limits (no monorepo support, cross-unit imports
counted rather than resolved).

- [ ] **Step 6: Commit**

```bash
git add docs/specs/SPEC-triple-store-rete.md crates/phronesis-mcp/CLAUDE.md CHANGELOG.md
git commit -m "docs(graph): record TypeScript corpus measurement and coverage"
```

---

## Self-Review

**Spec coverage.** Identity → Tasks 1, 2, 5. Discovery incl. `node_modules` and `extends` → Task 4. Resolution incl. the unresolved-counting rule → Tasks 5, 6. Relations incl. `tested_by` titles and the `!` watchlist → Tasks 6, 7. Both rules → Task 9. Out-of-scope items are asserted nowhere, correctly — cross-unit imports fall through to `Resolution::External` or `Unresolved` and are counted, never edged. Testing incl. the real-project gate → Tasks 10, 11.

**Type consistency.** `UnitContext` gains `ts: TsConfig` and `files: Vec<String>` in Task 4 and is constructed with exactly those fields in the Task 5 and 6 test helpers. `Resolution` is defined in Task 5 and matched exhaustively in Task 6. `Extracted` keeps its three existing fields; Task 6 uses `Extracted::unparseable()` rather than an empty edge set, per the global constraints.

**Known softness, flagged rather than hidden.** The tree-sitter node kinds in Tasks 6 and 7 (`non_null_expression`, `method_definition`, `variable_declarator`, `member_expression`) are from the `tree-sitter-typescript` grammar and should be confirmed against the installed version at implementation time — if a kind is wrong the test fails loudly, which is the right failure. The Task 8 assertion accepts two identity spellings because `module_base` for a unit at the repo root depends on whether `baseUrl` is set; the Task 5 tests pin the exact behaviour, so this looseness costs nothing.
