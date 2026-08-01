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

    let mut best: Option<(&String, &Vec<String>, usize)> = None;
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
        if specifier.starts_with(prefix) && best.is_none_or(|(_, _, len)| prefix.len() > len) {
            best = Some((alias, targets, prefix.len()));
        }
    }
    if let Some((_, targets, prefix_len)) = best {
        let rest = &specifier[prefix_len..];
        for target in targets {
            let candidate = with_base(&target.replace('*', rest), unit);
            if let Some(found) = probe(&candidate, unit) {
                return Resolution::File(found);
            }
        }
    }

    if (!unit.ts.base_url.is_empty() || !unit.module_base.is_empty())
        && let Some(found) = probe(&with_base(specifier, unit), unit)
    {
        return Resolution::File(found);
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
        assert_eq!(
            module_path("lib/util.ts", &c),
            "typescript:myapp::lib::util"
        );
    }

    #[test]
    fn a_tsx_extension_is_stripped_like_any_other() {
        let c = ctx(&["src/Button.tsx"], "src", &[]);
        assert_eq!(
            module_path("src/Button.tsx", &c),
            "typescript:myapp::Button"
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
}
