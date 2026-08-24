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
///
/// Deliberately *not* collapsing `billing/index.ts` onto `billing`, nor any
/// two of `.ts`/`.tsx`/`.mts`/`.cts`/`.d.ts` onto the same stem: every one of
/// these can be indexed in the same unit at once (a directory can hold both
/// `billing.ts` and `billing/index.ts`; a component can sit beside its
/// non-JSX helper as `Button.tsx` and `Button.ts`; a dual ESM/CJS build ships
/// `legacy.mts` next to `legacy.cts`; a `.d.ts` commonly sits beside its
/// `.ts`), and collapsing any such pair onto one identity would merge two
/// distinct files' definitions and edges into a single node — the exact
/// failure this identity scheme exists to prevent (see the module doc on
/// `unit.rs`). `strip_known_extension` therefore strips only a literal
/// trailing `.ts` and leaves every other extension as part of the final
/// segment: `x.ts` -> `x`, but `x.tsx`, `x.mts`, `x.cts` are untouched, and
/// `x.d.ts` -> `x.d` (only its trailing `.ts` disappears).
pub fn module_path(file_rel: &str, unit: &UnitContext) -> String {
    let rel = strip_module_base(file_rel, &unit.module_base);
    let trimmed = strip_known_extension(rel);
    let segments: Vec<&str> = trimmed.split('/').filter(|s| !s.is_empty()).collect();
    std::iter::once(unit.id.as_str())
        .chain(segments)
        .collect::<Vec<_>>()
        .join("::")
}

fn strip_module_base<'a>(file_rel: &'a str, module_base: &str) -> &'a str {
    // Defensive against a trailing `/` in `module_base` regardless of what
    // the caller hands in — `join_rel` no longer produces one, but this
    // function has no business trusting that formatting invariant either.
    let module_base = module_base.trim_end_matches('/');
    if module_base.is_empty() {
        return file_rel;
    }
    file_rel
        .strip_prefix(module_base)
        .and_then(|rest| rest.strip_prefix('/'))
        .unwrap_or(file_rel)
}

fn strip_known_extension(path: &str) -> &str {
    // Only a literal trailing `.ts` is stripped. Stripping `.tsx`/`.mts`/
    // `.cts` as well would collide two files that are commonly indexed side
    // by side (see the doc comment on `module_path`) onto one identity;
    // leaving them as part of the final segment keeps every extension
    // distinguishable. `.d.ts` still reduces to `x.d`, since only its
    // trailing `.ts` matches.
    path.strip_suffix(".ts").unwrap_or(path)
}

/// Resolve one import specifier from `importing_file`.
pub fn resolve_specifier(specifier: &str, importing_file: &str, unit: &UnitContext) -> Resolution {
    if specifier.starts_with('.') {
        let dir = importing_file.rsplit_once('/').map_or("", |(d, _)| d);
        // A `..` that climbs above the repo root has nowhere to go: it must
        // not silently collapse to a path that happens to collide with some
        // unrelated file at the root.
        return match normalize(&format!("{dir}/{specifier}")) {
            Some(joined) => match probe(&joined, unit) {
                Some(found) => Resolution::File(found),
                // Relative means "inside this project" by definition.
                None => Resolution::Unresolved,
            },
            None => Resolution::Unresolved,
        };
    }

    // Whether some alias that unambiguously names project-local code — a
    // non-wildcard exact alias, or a wildcard alias with a non-empty prefix
    // — matched `specifier` against at least one target that is not itself
    // an explicit detour through `node_modules`. A bare `"*"` alias is
    // excluded even though it "matches" everything: it also matches every
    // third-party specifier by construction (a common tsconfig shape routes
    // untyped third-party imports through a `types/*` fallback), so a miss
    // under it carries no more signal than an ordinary unmatched bare
    // specifier and must not drown real misses in noise. Checked only after
    // every resolution path — including the `baseUrl` fallback below — has
    // had its turn: a miss on one alias must not pre-empt a later, looser
    // alias or the plain `baseUrl` resolution that TypeScript itself falls
    // back to.
    let mut alias_matched_inside_project = false;

    for (alias, targets) in &unit.ts.paths {
        if alias.strip_suffix('*').is_none() && alias == specifier {
            for target in targets {
                if points_into_node_modules(target) {
                    continue;
                }
                alias_matched_inside_project = true;
                if let Some(found) = probe(&with_base(target, unit), unit) {
                    return Resolution::File(found);
                }
            }
        }
    }

    let mut best: Option<(&Vec<String>, usize)> = None;
    for (alias, targets) in &unit.ts.paths {
        let Some(prefix) = alias.strip_suffix('*') else {
            continue;
        };
        if specifier.starts_with(prefix) && best.is_none_or(|(_, len)| prefix.len() > len) {
            best = Some((targets, prefix.len()));
        }
    }
    if let Some((targets, prefix_len)) = best {
        let rest = &specifier[prefix_len..];
        for target in targets {
            let substituted = target.replace('*', rest);
            if points_into_node_modules(&substituted) {
                continue;
            }
            if prefix_len > 0 {
                alias_matched_inside_project = true;
            }
            if let Some(found) = probe(&with_base(&substituted, unit), unit) {
                return Resolution::File(found);
            }
        }
    }

    if (!unit.ts.base_url.is_empty() || !unit.module_base.is_empty())
        && let Some(found) = probe(&with_base(specifier, unit), unit)
    {
        return Resolution::File(found);
    }

    if alias_matched_inside_project {
        // A specific project alias matched but every target missed, and
        // baseUrl resolution also missed. This is our bug, not a
        // third-party import, and must stay counted.
        return Resolution::Unresolved;
    }

    Resolution::External
}

/// Whether `path` routes through `node_modules` — an explicit detour to
/// third-party code rather than a mapping onto this project's own files.
fn points_into_node_modules(path: &str) -> bool {
    path.split('/').any(|segment| segment == "node_modules")
}

/// Join a unit-relative path onto the module base (`baseUrl`).
fn with_base(path: &str, unit: &UnitContext) -> String {
    // Defensive against a trailing `/` on `module_base` for the same reason
    // as `strip_module_base`: a doubled slash matches no indexed file and
    // silently drops every alias in the unit.
    let base = unit.module_base.trim_end_matches('/');
    if base.is_empty() {
        path.to_string()
    } else {
        format!("{base}/{path}")
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

/// Collapse `.` and `..` segments in a path, or `None` if a `..` climbs
/// above the root.
///
/// Popping silently when `out` is already empty would let `../x` from a
/// root-level file collapse to plain `x` — a different file from what the
/// specifier names, and one that can accidentally exist. Reporting the
/// escape instead keeps a broken relative import `Unresolved` rather than
/// letting it resolve to an unrelated sibling.
fn normalize(path: &str) -> Option<String> {
    let mut out: Vec<&str> = Vec::new();
    for segment in path.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                out.pop()?;
            }
            s => out.push(s),
        }
    }
    Some(out.join("/"))
}

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
                        (
                            (*k).to_string(),
                            v.iter().map(|s| (*s).to_string()).collect(),
                        )
                    })
                    .collect(),
            },
            files: files.iter().map(|f| (*f).to_string()).collect(),
            lua_files: Vec::new(),
            cue_files: Vec::new(),
            test_target: false,
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
    fn an_index_file_keeps_its_own_segment() {
        // Not collapsed onto its directory's identity: a unit can index both
        // `billing.ts` and `billing/index.ts` at once (see
        // `a_file_wins_over_a_directory_of_the_same_name` below), and
        // collapsing the latter onto `billing` would merge two distinct
        // files' definitions and edges into one node.
        let c = ctx(&["src/billing/index.ts"], "src", &[]);
        assert_eq!(
            module_path("src/billing/index.ts", &c),
            "typescript:myapp::billing::index"
        );
    }

    #[test]
    fn without_a_base_url_the_unit_root_is_the_module_root() {
        let c = ctx(&["lib/util.ts"], "", &[]);
        assert_eq!(
            module_path("lib/util.ts", &c),
            "typescript:myapp::lib::util"
        );
    }

    #[test]
    fn a_tsx_extension_is_kept_intact_in_the_identity() {
        // Not stripped like `.ts`: a component's `Button.tsx` can sit beside
        // a non-JSX `Button.ts` in the same unit, and stripping both down to
        // `Button` would merge two distinct files onto one identity.
        let c = ctx(&["src/Button.tsx"], "src", &[]);
        assert_eq!(
            module_path("src/Button.tsx", &c),
            "typescript:myapp::Button.tsx"
        );
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
        assert_eq!(
            resolve_specifier("react", "src/a.ts", &c),
            Resolution::External
        );
    }

    #[test]
    fn a_scoped_third_party_specifier_is_external() {
        let c = ctx(&["src/a.ts"], "src", &[]);
        assert_eq!(
            resolve_specifier("@yourorg/shared", "src/a.ts", &c),
            Resolution::External
        );
    }

    #[test]
    fn the_most_specific_alias_wins_over_a_shorter_overlapping_one() {
        // `@app/*` and `@app/deep/*` both match `@app/deep/thing`; TypeScript
        // prefers the longest prefix, not whichever the BTreeMap iterates
        // first. `@app/*` is alphabetically and lexically earlier, so a
        // first-match-wins loop would pick it and land on the wrong file.
        let c = ctx(
            &["src/a.ts", "src/app/deep/thing.ts", "src/app/deep/other.ts"],
            "src",
            &[
                ("@app/*", &["app/wrong/*"]),
                ("@app/deep/*", &["app/deep/*"]),
            ],
        );
        assert_eq!(
            resolve_specifier("@app/deep/thing", "src/a.ts", &c),
            Resolution::File("src/app/deep/thing.ts".to_string())
        );
    }

    // ─── path normalization ─────────────────────────────────────────

    #[test]
    fn a_relative_specifier_that_climbs_above_the_root_is_unresolved() {
        // `..` from a top-level file has nothing to pop; it must not produce
        // a dangling `../x` that escapes the repo root or panics, and it
        // must not silently resolve to some unrelated file.
        let c = ctx(&["util.ts"], "", &[]);
        assert_eq!(
            resolve_specifier("../util", "a.ts", &c),
            Resolution::Unresolved
        );
    }

    // ─── alias miss is our bug, not a third party (fix round 1, finding 2) ──

    #[test]
    fn a_wildcard_alias_that_matches_but_misses_every_target_is_unresolved() {
        // `@app/gone` matches the declared `@app/*` pattern — it names
        // something in this project by construction — so a miss is our bug,
        // not a third-party import, and must stay counted.
        let c = ctx(
            &["src/a.ts", "src/app/billing.ts"],
            "src",
            &[("@app/*", &["app/*"])],
        );
        assert_eq!(
            resolve_specifier("@app/gone", "src/a.ts", &c),
            Resolution::Unresolved
        );
    }

    #[test]
    fn an_exact_alias_that_matches_but_misses_every_target_is_unresolved() {
        let c = ctx(&["src/a.ts"], "src", &[("@shim", &["shims/shim"])]);
        assert_eq!(
            resolve_specifier("@shim", "src/a.ts", &c),
            Resolution::Unresolved
        );
    }

    // ─── an alias miss must not short-circuit later resolution (fix round 2, finding 1) ──

    #[test]
    fn an_exact_alias_miss_falls_back_to_base_url() {
        // TypeScript falls back to baseUrl-relative resolution when a
        // `paths` mapping misses; a miss on `util` must not prevent
        // `src/util.ts` (found via baseUrl) from resolving.
        let c = ctx(
            &["src/a.ts", "src/util.ts"],
            "src",
            &[("util", &["shims/util"])],
        );
        assert_eq!(
            resolve_specifier("util", "src/a.ts", &c),
            Resolution::File("src/util.ts".to_string())
        );
    }

    #[test]
    fn a_wildcard_alias_miss_falls_back_to_base_url() {
        let c = ctx(
            &["src/a.ts", "src/lib/util.ts"],
            "src",
            &[("lib/*", &["shims/*"])],
        );
        assert_eq!(
            resolve_specifier("lib/util", "src/a.ts", &c),
            Resolution::File("src/lib/util.ts".to_string())
        );
    }

    #[test]
    fn a_missed_exact_alias_does_not_prevent_a_looser_wildcard_alias() {
        let c = ctx(
            &["src/a.ts", "src/real/thing.ts"],
            "src",
            &[("thing", &["nope/thing"]), ("thin*", &["real/thin*"])],
        );
        assert_eq!(
            resolve_specifier("thing", "src/a.ts", &c),
            Resolution::File("src/real/thing.ts".to_string())
        );
    }

    // ─── a declared alias pointing at third-party code is not our bug (fix round 2, finding 2) ──

    #[test]
    fn a_catch_all_wildcard_alias_still_reports_a_third_party_miss_as_external() {
        // `"*"` matches every bare specifier, including third-party ones —
        // it carries no more signal than an ordinary unmatched specifier, so
        // a miss under it must not manufacture Unresolved noise for every
        // third-party import in the project.
        let c = ctx(&["src/a.ts"], "src", &[("*", &["types/*", "*"])]);
        assert_eq!(
            resolve_specifier("react", "src/a.ts", &c),
            Resolution::External
        );
        assert_eq!(
            resolve_specifier("@yourorg/shared", "src/a.ts", &c),
            Resolution::External
        );
    }

    #[test]
    fn an_alias_explicitly_routed_through_node_modules_is_external_on_a_miss() {
        let c = ctx(
            &["src/a.ts"],
            "src",
            &[("react", &["../node_modules/react"])],
        );
        assert_eq!(
            resolve_specifier("react", "src/a.ts", &c),
            Resolution::External
        );
    }

    // ─── module_path is injective over the file set (fix round 1, finding 3) ──

    #[test]
    fn distinct_files_never_share_a_module_identity() {
        // Enumerates every extension `lang_of_path` accepts as TypeScript
        // (`.ts`, `.tsx`, `.mts`, `.cts`; `.d.ts` is a `.ts` file sharing a
        // stem with `x.ts`) plus the index/directory collision, so this test
        // actually verifies the injectivity it claims rather than a subset
        // of it.
        let files = [
            "src/x.ts",
            "src/x.tsx",
            "src/x.mts",
            "src/x.cts",
            "src/x.d.ts",
            "src/billing.ts",
            "src/billing/index.ts",
        ];
        let c = ctx(&files, "src", &[]);
        let mut seen = std::collections::BTreeMap::new();
        for file in files {
            let id = module_path(file, &c);
            if let Some(prev) = seen.insert(id.clone(), file) {
                panic!("{file} collided with {prev} onto {id}");
            }
        }
    }

    #[test]
    fn a_d_ts_file_is_distinct_from_its_ts_counterpart() {
        let c = ctx(&["src/x.ts", "src/x.d.ts"], "src", &[]);
        assert_ne!(module_path("src/x.ts", &c), module_path("src/x.d.ts", &c));
    }

    #[test]
    fn a_directory_index_is_distinct_from_a_same_named_file() {
        let c = ctx(&["src/billing.ts", "src/billing/index.ts"], "src", &[]);
        assert_ne!(
            module_path("src/billing.ts", &c),
            module_path("src/billing/index.ts", &c)
        );
    }

    // ─── trailing slash from an empty baseUrl (fix round 1, finding 1) ──────

    #[test]
    fn a_nested_unit_with_declared_paths_and_no_base_url_resolves_aliases() {
        use crate::graph::unit::UnitMap;
        use tempfile::TempDir;

        let d = TempDir::new().expect("tempdir");
        let write = |rel: &str, body: &str| {
            let p = d.path().join(rel);
            std::fs::create_dir_all(p.parent().expect("parent")).expect("mkdir");
            std::fs::write(p, body).expect("write");
        };
        write("packages/web/package.json", r#"{"name": "web"}"#);
        write(
            "packages/web/tsconfig.json",
            r#"{"compilerOptions": {"paths": {"@app/*": ["app/*"]}}}"#,
        );
        write("packages/web/a.ts", "");
        write("packages/web/app/billing.ts", "");

        let map = UnitMap::discover(d.path());
        let ctx = map.context_for("packages/web/a.ts");

        // No leaked root: an empty baseUrl must not leave a trailing slash
        // in module_base for with_base/strip_module_base to trip over.
        assert_eq!(ctx.module_base, "packages/web");

        assert_eq!(
            resolve_specifier("@app/billing", "packages/web/a.ts", &ctx),
            Resolution::File("packages/web/app/billing.ts".to_string())
        );
        assert_eq!(
            module_path("packages/web/app/billing.ts", &ctx),
            "typescript:web::app::billing"
        );
    }
}
