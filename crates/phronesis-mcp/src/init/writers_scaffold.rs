use std::path::Path;

use super::json_helpers::*;
use super::types::*;

/// Default `.phronesis/durable.md` template.
///
/// Without `.phronesis/context.json` this file is injected at every
/// SessionStart *and* every UserPromptSubmit — the whole thing, every
/// turn. With the context pack it becomes the session charter, rendered
/// at SessionStart only, and `kernel.md` carries the per-turn load.
///
/// Terse in both modes, for different reasons: in legacy mode every byte
/// is paid per turn, and in context-pack mode the charter competes with
/// the active-rule list for a shared budget. An earlier version of this
/// template ran to 3.3 KB, of which the participatory-governance section
/// (1825 B) exceeded `charter_max_bytes` and was dropped by
/// `kind_ceiling` on every session — never reaching the model at all,
/// while its bulk displaced rules. Reference material now lives in
/// `docs/participatory-governance.md`; only what the model must act on
/// belongs here. Measure with `phr-mcp context inspect --event session`
/// before adding to it.
pub(crate) const DEFAULT_DURABLE_MD: &str = r#"# Durable Directives

Rendered at SessionStart (and PostCompact). If `.phronesis/context.json`
is present, `.phronesis/kernel.md` carries the per-turn directives —
keep anything needed every turn there, not here. Budget is measured:
`phr-mcp context inspect --event session`.

## Drift discipline

`get_drift(source)` surfaces guidance that no rule enforces — `source` is
`claude_md`, `memory`, `wiki`, `code`, or `all`. Run it when the user asks
about rules, memory, or project conventions, or says "remember X" / "make
a rule for X".

Scoring is token-overlap Jaccard with no semantic match, so output is
a triage list, not ground truth.

## Participatory governance

Rule-evolution workflows — decision scaffolding, friction-driven
proposals, cross-session knowledge transfer — are in
`docs/participatory-governance.md`. Read it when proposing or
refining a rule.

## Project-specific guidance

Add team directives below. Keep them short: this file competes with
the active-rule list for the session budget.
"#;

const DEFAULT_CONTEXT_KERNEL: &str = r#"# Durable project kernel

- Treat Phronesis findings as evidence with stated limits, not proof.
- Obtain current build, test, manual, or traced evidence before claiming completion.
- Surface conflicts between guidance, rules, decisions, and implementation.
- Retrieve detailed rules, decisions, graph facts, and history over MCP when relevant.
"#;

pub(super) fn write_durable_md(
    root: &Path,
    opts: &InitOpts,
    report: &mut InitReport,
) -> Result<(), InitError> {
    let path = root.join(".phronesis").join("durable.md");

    if path.exists() {
        // Shipped templates are product-owned and migrate forward by default;
        // customized files remain byte-for-byte operator-owned.
        let note = match crate::durable_migrate::migrate(&path, opts.dry_run) {
            Ok(crate::durable_migrate::Outcome::Migrated) => {
                "+ migrated .phronesis/durable.md to the smaller current template \
                 (previous template saved as durable.md.bak)"
                    .to_string()
            }
            Ok(crate::durable_migrate::Outcome::WouldMigrate) => {
                "+ would migrate .phronesis/durable.md to the smaller current template".to_string()
            }
            Ok(crate::durable_migrate::Outcome::AlreadyCurrent) => {
                "= .phronesis/durable.md already uses the current template".to_string()
            }
            Ok(crate::durable_migrate::Outcome::SkippedCustomized) => {
                "= .phronesis/durable.md is customized — leaving unchanged".to_string()
            }
            Ok(crate::durable_migrate::Outcome::SkippedAbsent) => {
                "= .phronesis/durable.md disappeared before migration".to_string()
            }
            Err(e) => format!(
                "! .phronesis/durable.md exists but could not be read: {e} \
                 — check file permissions and retry"
            ),
        };
        report.steps.push(note);
        return Ok(());
    }

    // `durable.md` keeps its meaning whatever packs are selected: it is the
    // session-level project document. The default context support adds
    // `kernel.md` alongside it rather than replacing it.
    if opts.dry_run {
        report.steps.push(
            "+ would write .phronesis/durable.md (default drift-discipline notes)".to_string(),
        );
        return Ok(());
    }

    ensure_parent(&path)?;
    std::fs::write(&path, DEFAULT_DURABLE_MD).map_err(|e| InitError::Io {
        path: path.display().to_string(),
        source: e,
    })?;
    report
        .steps
        .push("+ wrote .phronesis/durable.md (default drift-discipline notes)".to_string());
    Ok(())
}

const CONTEXT_NUDGES_README: &str = r#"# Situational context nudges

Each `*.md` file in this directory is a **capsule**: a short piece of trusted
project guidance that is injected into the model's context only when current
facts make it relevant. Files load in bytewise filename order.

A capsule is strict JSON frontmatter delimited by `---json` and a line of
exactly `---`, followed by a static Markdown body:

```markdown
---json
{
  "id": "low-grounded-confidence",
  "priority": 95,
  "max_bytes": 240,
  "when": {
    "predicate": "context_confidence_band",
    "args": ["low"]
  }
}
---

Grounded confidence for the open work unit is low. Obtain current build, test,
or known-bug evidence before making an irreversible claim or operation.
```

## Frontmatter

| Field | Type | Contract |
|---|---|---|
| `id` | string | `[a-z0-9][a-z0-9-]{0,63}`, unique across the project |
| `priority` | integer | 0–100 inclusive; higher packs first |
| `max_bytes` | integer | 64–1024 inclusive; an assertion about *this* body |
| `when` | condition | positive condition tree, see below |

Parsing is deliberately unforgiving, because this is a prompt surface:
duplicate keys at any nesting level, unknown fields, wrong scalar types,
trailing JSON after the object, and non-object roots are all errors. A
duplicate `id` skips **every** file sharing it — no copy wins. A body larger
than its declared `max_bytes` is rejected at load time rather than silently
competing under a false assertion.

`max_bytes` bounds one capsule. Competition *among* capsules is governed by
`interaction.nudges_max_bytes` in `.phronesis/context.json`.

## Conditions

`when` is either a single leaf or a positive `all` / `any` group:

```json
{"when": {"predicate": "context_confidence_band", "args": ["low"]}}

{"when": {"all": [
  {"predicate": "journey_seen", "args": ["rule-blocked", "s"]},
  {"predicate": "context_confidence_band", "args": ["low"]}
]}}
```

- `args` are exact constant matches. Variables (`?name`) are rejected —
  the body cannot use bindings, so a binding would buy nothing.
- `all` and `any` nest, up to 16 levels and 256 expanded alternatives.
- `journey_*` window args are `s` (this session), `<N>c` (last N tool
  calls), or `<N>s`/`<N>m`/`<N>h`/`<N>d`. Lifecycle selectors need `s` or a
  time window. `inspect` reports a bad window token as a derivation error.
- `not`, `unless`, empty groups, scripts, and actions are rejected. There is
  no absence-based trigger: a capsule fires on facts that are true, never on
  facts that are missing.

Only allowlisted predicates may trigger a capsule. Run
`phr-mcp context predicates` for the current list — adding to it is a
reviewed code change, not a configuration option.

## Bodies are static

A capsule body is literal text. Runtime facts select a capsule but are never
interpolated into it, which is what stops a filename, tool output, or rule
parameter from becoming a second-order prompt-injection channel. `?variable`,
`{{ ... }}`, and `${ ... }` are rejected so no author is misled into thinking
substitution happens.

The renderer appends only the capsule's own trusted id:

```text
[phronesis nudge: low-grounded-confidence]
```

Keep detailed procedures in ADRs or project docs — a capsule may name a stable
MCP tool or decision id in prose, but it is a short situational reminder, not
documentation.

## Checking your work

```sh
phr-mcp context predicates                    # what may trigger a capsule
phr-mcp context inspect --event interaction   # dry run: what would be selected, and why not
phr-mcp context stats --since 7d              # observed cost and selection counts
```

`inspect` is read-only: it writes no observation, so inspecting never
contaminates the data it reports on. It names every capsule that failed to
load and every demanded fact that could not be hydrated — which is how you
tell "the facts were false" apart from "the selector has a typo."
"#;

pub(super) fn write_context_scaffold(
    root: &Path,
    opts: &InitOpts,
    report: &mut InitReport,
) -> Result<(), InitError> {
    if !opts.packs.contains(&Pack::Context) {
        return Ok(());
    }
    let config_path = root.join(".phronesis").join("context.json");
    let kernel_path = root.join(".phronesis").join("kernel.md");
    let readme_path = root.join(".phronesis").join("nudges").join("README.md");
    if opts.dry_run {
        if !config_path.exists() {
            report
                .steps
                .push("+ would write .phronesis/context.json".to_string());
        }
        if !kernel_path.exists() {
            report
                .steps
                .push("+ would write .phronesis/kernel.md".to_string());
        }
        if !readme_path.exists() {
            report
                .steps
                .push("+ would write .phronesis/nudges/README.md".to_string());
        }
        return Ok(());
    }
    if !kernel_path.exists() {
        ensure_parent(&kernel_path)?;
        std::fs::write(&kernel_path, DEFAULT_CONTEXT_KERNEL).map_err(|source| InitError::Io {
            path: kernel_path.display().to_string(),
            source,
        })?;
        report
            .steps
            .push("+ wrote .phronesis/kernel.md (always-on kernel)".to_string());
    } else {
        report
            .steps
            .push("= .phronesis/kernel.md already exists — leaving unchanged".to_string());
    }
    if !config_path.exists() {
        ensure_parent(&config_path)?;
        let body = serde_json::to_string_pretty(&crate::context::config::ContextConfig::default())
            .map_err(InitError::Json)?;
        std::fs::write(&config_path, format!("{body}\n")).map_err(|source| InitError::Io {
            path: config_path.display().to_string(),
            source,
        })?;
        report
            .steps
            .push("+ wrote .phronesis/context.json".to_string());
    } else {
        report
            .steps
            .push("= .phronesis/context.json already exists — leaving unchanged".to_string());
    }
    if !readme_path.exists() {
        ensure_parent(&readme_path)?;
        std::fs::write(&readme_path, CONTEXT_NUDGES_README).map_err(|source| InitError::Io {
            path: readme_path.display().to_string(),
            source,
        })?;
        report
            .steps
            .push("+ wrote .phronesis/nudges/README.md".to_string());
    } else {
        report
            .steps
            .push("= .phronesis/nudges/README.md already exists — leaving unchanged".to_string());
    }
    Ok(())
}

const WIKI_DECISIONS_README: &str = "\
# `.phronesis/wiki/decisions/`

ADR-style decision pages. Each file is one decision (e.g. \
`2026-05-29-error-handling-policy.md`). The first block is YAML \
frontmatter (`id`, `date`, `status`, optional `enforces`, \
`superseded_by`, `tags`). The body uses Context / Decision / \
Enforcement / Consequences sections.

Run `phr-mcp drift --source wiki` to see which decisions lack rule coverage.
Create new pages with `phr-mcp decision new <slug>`.

This directory is tracked in git (un-ignored from the broader \
`.phronesis/` ignore) because decisions are project knowledge. \
The rest of `.phronesis/` (rules.json, log.jsonl, etc.) stays \
gitignored.
";

pub(super) fn write_wiki_scaffold(
    root: &Path,
    opts: &InitOpts,
    report: &mut InitReport,
) -> Result<(), InitError> {
    let dir = root.join(".phronesis").join("wiki").join("decisions");
    let readme = dir.join("README.md");

    if readme.exists() {
        report.steps.push(
            "= .phronesis/wiki/decisions/README.md already exists — leaving unchanged".to_string(),
        );
        return Ok(());
    }

    if opts.dry_run {
        report
            .steps
            .push("+ would create .phronesis/wiki/decisions/ + README.md".to_string());
        return Ok(());
    }

    std::fs::create_dir_all(&dir).map_err(|e| InitError::Io {
        path: dir.display().to_string(),
        source: e,
    })?;
    std::fs::write(&readme, WIKI_DECISIONS_README).map_err(|e| InitError::Io {
        path: readme.display().to_string(),
        source: e,
    })?;
    report
        .steps
        .push("+ created .phronesis/wiki/decisions/ + README.md".to_string());
    Ok(())
}

/// Default `.phronesis/confidence.json` configuration that activates
/// confidence scoring for the project.
const CONFIDENCE_JSON: &str = "{\n  \"version\": 1\n}\n";

/// Default `.phronesis/bugs.json` — the known-bug registry, empty to start.
/// Each entry: `{ "bug_id": "...", "test": "module::test_name", "status": "open" }`.
const CONFIDENCE_BUGS_JSON: &str = "[]\n";

/// Example `.phronesis/toolchains.json` written with the confidence pack:
/// two real project-def examples (pytest, tsc) proving toolchain neutrality.
/// Matchers are head-anchored: recognition runs per command segment (split
/// on `&&`, `||`, `;`, `|`, newlines; leading env assignments stripped), so
/// `^pytest` matches `cd api && pytest -q` but not `echo pytest`.
/// Users edit/extend in place; `init` never overwrites it.
const TOOLCHAINS_JSON: &str = r#"[
  {
    "_doc": "Recognition regex is `matches`, applied to each command segment (split on &&, ||, ;, |, newlines; leading NAME=value and `env` prefixes stripped) — anchor with ^ to match only real invocations. Refinement fields are optional; `compile_success` lists explicit success evidence used when no exit code was captured (unused by these examples; the built-in cargo def uses it). See `phr-mcp toolchains`.",
    "id": "pytest",
    "matches": "^(python3? -m )?pytest(\\s|$)",
    "compile_fail": ["SyntaxError", "ImportError"],
    "test_summary": "(?P<failed>\\d+) failed|(?P<passed>\\d+) passed",
    "per_test": "(?m)^(?P<name>\\S+) (?P<status>PASSED|FAILED)",
    "pass_tokens": ["PASSED"]
  },
  {
    "id": "tsc",
    "matches": "^(npx )?tsc(\\s|$)",
    "compile_fail": ["error TS\\d+"]
  }
]
"#;

/// Write the confidence configuration, known-bug registry, and
/// toolchains.json example when the `confidence` pack is selected. Idempotent (leaves existing files alone).
pub(super) fn write_confidence_scaffold(
    root: &Path,
    opts: &InitOpts,
    report: &mut InitReport,
) -> Result<(), InitError> {
    if !opts.packs.contains(&Pack::Confidence) {
        return Ok(());
    }
    let phr = root.join(".phronesis");
    for (name, contents) in [
        ("confidence.json", CONFIDENCE_JSON),
        ("bugs.json", CONFIDENCE_BUGS_JSON),
        ("toolchains.json", TOOLCHAINS_JSON),
    ] {
        let path = phr.join(name);
        if path.exists() {
            report.steps.push(format!(
                "= .phronesis/{name} already exists — leaving unchanged"
            ));
            continue;
        }
        if opts.dry_run {
            report
                .steps
                .push(format!("+ would create .phronesis/{name}"));
            continue;
        }
        std::fs::create_dir_all(&phr).map_err(|e| InitError::Io {
            path: phr.display().to_string(),
            source: e,
        })?;
        std::fs::write(&path, contents).map_err(|e| InitError::Io {
            path: path.display().to_string(),
            source: e,
        })?;
        report.steps.push(format!("+ created .phronesis/{name}"));
    }
    Ok(())
}

/// Toolchain def for `cue vet` / `cue eval` / `cue export` / `cue fmt --check`.
///
/// `compile_fail` patterns pinned from live `cue vet` output on this machine
/// (cue v0.16.1: `a: int` + `a: "hello"` → `a: conflicting values int and
/// "hello" (mismatched types int and string):`; `value: missingRef` →
/// `value: reference "missingRef" not found:`; `cue vet -c` over
/// `instance: {age: int}` → `instance.age: incomplete value int:`;
/// `strings.HasPrefix(42, "x")` → `result: cannot use 42 (type int) as
/// string in argument 1 to strings.HasPrefix:`; every diagnostic is followed
/// by location lines like `    ./conflict.cue:1:4`). The `(?m)^` line anchor
/// follows the codebase convention: `compile_fail` regexes run against the
/// whole output, so a bare `^` would anchor only at output start. A passing
/// `cue vet` (exit 0, no diagnostic) grounds a `compile` pass; `cue fmt
/// --check` failure prints only the file name, so its failure is grounded by
/// the exit code, not text.
const CUE_TOOLCHAIN_JSON: &str = r#"{
  "id": "cue",
  "matches": "^cue\\s+(vet|eval|export|fmt\\s+--check)(\\s|$)",
  "compile_fail": [
    "(?m)^.*: (conflicting values|incomplete value|reference .* not found|cannot use)",
    "\\.cue:\\d+:\\d+"
  ]
}"#;

/// Toolchain def for `helm lint` / `helm template`.
///
/// UNVERIFIED: `helm` is not installed on the development machine, so the
/// `compile_fail` and `test_summary` patterns are pinned from the plan's
/// documented shapes, not from a live `helm lint` run. A reviewer with
/// helm installed should confirm them.
///
/// `compile_fail`:
/// - `[ERROR]` — helm lint error prefix
/// - `Error:` — helm general error prefix
///
/// `test_summary`:
/// - `(?P<passed>\d+) chart\(s\) linted, (?P<failed>\d+) chart\(s\) failed`
const HELM_TOOLCHAIN_JSON: &str = r#"{
  "id": "helm",
  "matches": "^helm\\s+(lint|template)(\\s|$)",
  "compile_fail": ["\\[ERROR\\]", "Error:"],
  "test_summary": "(?P<passed>\\d+) chart\\(s\\) linted, (?P<failed>\\d+) chart\\(s\\) failed"
}"#;

/// Write language-pack toolchain definitions for the selected packs, using
/// merge-if-absent semantics: missing toolchain ids are added to
/// `.phronesis/toolchains.json`, existing ids are never touched.
///
/// This is the language-pack toolchains writer (plan Part L decision 1 /
/// H decision 7): one mechanism, shared by every language pack that ships
/// a toolchain def. The confidence pack's `write_confidence_scaffold` writes
/// the example `toolchains.json` from scratch (skip-if-exists); this writer
/// merges into whatever is already there (add missing ids only).
pub(super) fn write_language_pack_toolchains(
    root: &Path,
    opts: &InitOpts,
    report: &mut InitReport,
) -> Result<(), InitError> {
    let mut defs_to_add: Vec<(&str, &str)> = Vec::new();
    if opts.packs.contains(&Pack::Cue) {
        defs_to_add.push(("cue", CUE_TOOLCHAIN_JSON));
    }
    if opts.packs.contains(&Pack::Helm3) {
        defs_to_add.push(("helm", HELM_TOOLCHAIN_JSON));
    }
    if defs_to_add.is_empty() {
        return Ok(());
    }

    let phr = root.join(".phronesis");
    let path = crate::outcomes::toolchain::config_path(root);

    // Read existing file (if any) as a JSON array; a non-array file is a
    // data error that fails init — the human is present at init time and a
    // broken file must not be silently extended.
    let existing: Vec<serde_json::Value> = match std::fs::read_to_string(&path) {
        Ok(content) => match serde_json::from_str(&content) {
            Ok(v) => v,
            Err(_) => {
                return Err(InitError::InvalidRules(
                    ".phronesis/toolchains.json must be a JSON array".into(),
                ));
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => {
            return Err(InitError::Io {
                path: path.display().to_string(),
                source: e,
            });
        }
    };

    let existing_ids: std::collections::BTreeSet<String> = existing
        .iter()
        .filter_map(|v| v.get("id").and_then(|i| i.as_str()).map(String::from))
        .collect();

    let mut merged = existing;
    let mut added = Vec::new();
    for (id, json) in &defs_to_add {
        if existing_ids.contains(*id) {
            continue;
        }
        let val: serde_json::Value = serde_json::from_str(json).map_err(InitError::Json)?;
        merged.push(val);
        added.push(*id);
    }

    if added.is_empty() {
        report.steps.push(
            "= .phronesis/toolchains.json already has all language-pack defs — leaving unchanged"
                .to_string(),
        );
        return Ok(());
    }

    if opts.dry_run {
        for id in &added {
            report.steps.push(format!(
                "+ would add `{id}` toolchain to .phronesis/toolchains.json"
            ));
        }
        return Ok(());
    }

    std::fs::create_dir_all(&phr).map_err(|e| InitError::Io {
        path: phr.display().to_string(),
        source: e,
    })?;
    let body = serde_json::to_string_pretty(&merged).map_err(InitError::Json)?;
    std::fs::write(&path, format!("{body}\n")).map_err(|e| InitError::Io {
        path: path.display().to_string(),
        source: e,
    })?;

    for id in &added {
        report.steps.push(format!(
            "+ added `{id}` toolchain to .phronesis/toolchains.json"
        ));
    }
    Ok(())
}

pub(super) fn write_java_toolchains(
    root: &Path,
    opts: &InitOpts,
    report: &mut InitReport,
) -> Result<(), InitError> {
    if !opts.packs.contains(&Pack::Java) {
        return Ok(());
    }
    let path = root.join(".phronesis/toolchains.json");
    let mut defs: serde_json::Value = if path.exists() {
        serde_json::from_slice(&std::fs::read(&path).map_err(|source| InitError::Io {
            path: path.display().to_string(),
            source,
        })?)?
    } else {
        serde_json::json!([])
    };
    let array = defs
        .as_array_mut()
        .ok_or_else(|| InitError::InvalidRules("toolchains.json must be an array".into()))?;
    for def in [
        serde_json::json!({"id":"mvn","matches":"^(\\./)?mvnw?(\\s|$)","compile_fail":"COMPILATION ERROR|\\[ERROR\\] .*\\.java","test_summary":"Tests run: (?P<total>\\d+), Failures: (?P<failed>\\d+), Errors: (?P<errors>\\d+)"}),
        serde_json::json!({"id":"gradle","matches":"^(\\./)?gradlew?(\\s|$)","compile_fail":"error: |Compilation failed","test_summary":"(?P<total>\\d+) tests completed, (?P<failed>\\d+) failed","compile_success":"BUILD SUCCESSFUL"}),
    ] {
        let id = def["id"].as_str().unwrap_or_default();
        if !array
            .iter()
            .any(|existing| existing["id"].as_str() == Some(id))
        {
            array.push(def);
        }
    }
    if opts.dry_run {
        report
            .steps
            .push("+ would merge mvn and gradle toolchains".into());
        return Ok(());
    }
    std::fs::create_dir_all(path.parent().expect("toolchain parent")).map_err(|source| {
        InitError::Io {
            path: path.display().to_string(),
            source,
        }
    })?;
    let bytes = serde_json::to_vec_pretty(&defs)?;
    std::fs::write(&path, bytes).map_err(|source| InitError::Io {
        path: path.display().to_string(),
        source,
    })?;
    report
        .steps
        .push("+ merged mvn and gradle toolchains".into());
    Ok(())
}

/// Starter `.phronesis/journey.json` — schema version, one example tagger
/// (`build` matches `cargo (build|check|test)`), empty `modules`. Authors
/// extend it with their project's risk surface (auth, sql, payments, …)
/// per SPEC-journey-facts §"The project-defined seam".
const JOURNEY_JSON: &str = r#"{
  "version": 1,
  "taggers": [
    { "tag": "build", "when": [ { "bash_command_matches": "cargo (build|check|test)" } ] }
  ],
  "modules": []
}
"#;

/// Write `.phronesis/journey.json` when the `journey` pack is selected.
/// Idempotent — leaves an existing file alone so a project's customized
/// tagger vocabulary isn't clobbered by a re-run.
pub(super) fn write_journey_scaffold(
    root: &Path,
    opts: &InitOpts,
    report: &mut InitReport,
) -> Result<(), InitError> {
    if !opts.packs.contains(&Pack::Journey) {
        return Ok(());
    }
    let phr = root.join(".phronesis");
    let path = phr.join("journey.json");
    if path.exists() {
        report
            .steps
            .push("= .phronesis/journey.json already exists — leaving unchanged".to_string());
        return Ok(());
    }
    if opts.dry_run {
        report
            .steps
            .push("+ would create .phronesis/journey.json".to_string());
        return Ok(());
    }
    std::fs::create_dir_all(&phr).map_err(|e| InitError::Io {
        path: phr.display().to_string(),
        source: e,
    })?;
    std::fs::write(&path, JOURNEY_JSON).map_err(|e| InitError::Io {
        path: path.display().to_string(),
        source: e,
    })?;
    report
        .steps
        .push("+ created .phronesis/journey.json".to_string());
    Ok(())
}

/// Build `.phronesis/graph.jsonl` so the structural pack works immediately.
///
/// Without this the pack installs *silent*: the rules load, hydration finds an
/// empty graph, nothing ever matches, and the user reasonably concludes the
/// feature is broken rather than unbuilt. A rule that cannot fire is
/// indistinguishable from a rule that found nothing.
///
/// Deliberately infallible. Writing hook config and rules is what `init`
/// exists to do; a graph that cannot be built is fully recoverable with
/// `phr-mcp graph rebuild`, whereas a failed `init` leaves a project with no
/// enforcement at all. Failures are reported as warnings and named, so the
/// user knows to run the rebuild by hand.
pub(super) fn build_structural_graph(root: &Path, opts: &InitOpts, report: &mut InitReport) {
    if opts.dry_run {
        report
            .steps
            .push("+ would build .phronesis/graph.jsonl (structural graph)".to_string());
        return;
    }
    match crate::graph::sync::rebuild(root) {
        Ok(out) => report.steps.push(format!(
            "+ built .phronesis/graph.jsonl ({} edges, {} derived)",
            out.base, out.derived
        )),
        Err(e) => report.warnings.push(format!(
            "could not build the structural graph ({e}). \
             Structural rules stay silent until you run `phr-mcp graph rebuild`."
        )),
    }
}

pub(super) fn update_gitignore(
    root: &Path,
    opts: &InitOpts,
    report: &mut InitReport,
) -> Result<(), InitError> {
    let path = root.join(".gitignore");
    let mut entries = vec![
        ".phronesis/log.jsonl",
        ".phronesis/log.jsonl.1",
        ".phronesis/rules.json.bak",
        // Broad ignore of .phronesis/ contents, then carve the wiki tree
        // back in. `.phronesis/*` (with the trailing `*`) — NOT
        // `.phronesis/` — because the latter prevents git from listing
        // the dir at all, making the un-ignore inert. Order matters:
        // un-ignores must come after the broad ignore.
        ".phronesis/*",
        "!.phronesis/wiki/",
        "!.phronesis/wiki/**",
        // Predicate providers are durable project policy and should travel
        // with the repository; runtime journals and outcomes remain ignored.
        "!.phronesis/predicates/",
        "!.phronesis/predicates/**",
    ];
    // Confidence config is project knowledge (track it); the per-subject
    // outcome ledger under .phronesis/outcomes/ stays ignored via `.phronesis/*`.
    if opts.packs.contains(&Pack::Confidence) {
        entries.push("!.phronesis/confidence.json");
        entries.push("!.phronesis/bugs.json");
        entries.push("!.phronesis/toolchains.json");
    }
    // Journey config is project knowledge (track it); the journal under
    // .phronesis/journey/ (events.jsonl, session, seq) stays ignored via
    // `.phronesis/*` — local state only.
    if opts.packs.contains(&Pack::Journey) {
        entries.push("!.phronesis/journey.json");
    }
    // Context configuration and nudge capsules are project-owned policy that
    // must travel with the repository. Without these carveouts the broad
    // `.phronesis/*` ignore would hide the very files the pack just wrote.
    if opts.packs.contains(&Pack::Context) {
        entries.push("!.phronesis/context.json");
        entries.push("!.phronesis/kernel.md");
        entries.push("!.phronesis/nudges/");
        entries.push("!.phronesis/nudges/**");
    }
    let original = if path.exists() {
        std::fs::read_to_string(&path).map_err(|e| InitError::Io {
            path: path.display().to_string(),
            source: e,
        })?
    } else {
        String::new()
    };

    // Migrate legacy bare `.phronesis/` to `.phronesis/*`. The bare form
    // tells git not to descend into the directory at all, which makes
    // any later `!.phronesis/wiki/**` un-ignore inert. Pre-0.9.0 init
    // wrote the bare form; rewrite it so the carveout takes effect.
    //
    // After rewriting, dedupe the lines we manage (broad-ignore and
    // un-ignore carveouts) preserving first occurrence — a project that
    // already had `.phronesis/*` *and* the legacy `.phronesis/` would
    // otherwise end up with two `.phronesis/*` lines side by side.
    let had_trailing_newline = original.ends_with('\n');
    let mut migrated_count = 0usize;
    let rewritten: Vec<String> = original
        .lines()
        .map(|line| {
            if line == ".phronesis/" {
                migrated_count += 1;
                ".phronesis/*".to_string()
            } else {
                line.to_string()
            }
        })
        .collect();

    let mut seen_managed: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut deduped_count = 0usize;
    let mut migrated_lines: Vec<String> = Vec::with_capacity(rewritten.len());
    for line in rewritten {
        let is_managed = line == ".phronesis/*" || line.starts_with("!.phronesis/");
        if is_managed && !seen_managed.insert(line.clone()) {
            deduped_count += 1;
            continue;
        }
        migrated_lines.push(line);
    }
    let mut migrated_content = migrated_lines.join("\n");
    if had_trailing_newline && !migrated_content.is_empty() {
        migrated_content.push('\n');
    }

    let present_lines: std::collections::HashSet<&str> = migrated_content.lines().collect();
    let missing: Vec<&str> = entries
        .iter()
        .filter(|e| !present_lines.contains(*e))
        .copied()
        .collect();

    if missing.is_empty() && migrated_count == 0 && deduped_count == 0 {
        report
            .steps
            .push("= .gitignore already contains phronesis entries".to_string());
        return Ok(());
    }

    if opts.dry_run {
        if migrated_count > 0 {
            report.steps.push(format!(
                "~ would migrate {} bare `.phronesis/` line(s) to `.phronesis/*`",
                migrated_count
            ));
        }
        if deduped_count > 0 {
            report.steps.push(format!(
                "~ would dedupe {} duplicate phronesis line(s)",
                deduped_count
            ));
        }
        if !missing.is_empty() {
            report.steps.push(format!(
                "+ would append {} line(s) to .gitignore: {:?}",
                missing.len(),
                missing
            ));
        }
        return Ok(());
    }

    let mut new_content = migrated_content;
    if !new_content.is_empty() && !new_content.ends_with('\n') {
        new_content.push('\n');
    }
    for line in &missing {
        new_content.push_str(line);
        new_content.push('\n');
    }
    std::fs::write(&path, new_content).map_err(|e| InitError::Io {
        path: path.display().to_string(),
        source: e,
    })?;
    if migrated_count > 0 {
        report.steps.push(format!(
            "~ migrated {} bare `.phronesis/` line(s) to `.phronesis/*` (carveout was inert)",
            migrated_count
        ));
    }
    if deduped_count > 0 {
        report.steps.push(format!(
            "~ removed {} duplicate phronesis line(s)",
            deduped_count
        ));
    }
    if !missing.is_empty() {
        report
            .steps
            .push(format!("+ updated .gitignore (+{} entries)", missing.len()));
    }
    Ok(())
}
