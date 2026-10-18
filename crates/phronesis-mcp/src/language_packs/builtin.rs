//! Built-in language packs shipped inside this crate, embedded with
//! `include_bytes!` so an installed binary needs no source checkout. Go
//! ships here — not as a bespoke extractor — running through the exact same
//! manifest/asset/query/Rhai pipeline (`language_packs::{assets, manifest,
//! extract}`) an externally installed pack does; nothing in this module is
//! a privileged shortcut.
//!
//! [`builtin_pack_sources`] is the seam [`super::registry::builtin_pack_sources`]
//! appends to: raw, unvalidated-beyond-manifest-shape assets, so a built-in
//! pack whose adapter needs a compiled-out feature (Go without
//! `go-grammar`) still enumerates — with `status: unsupported` — instead of
//! silently vanishing. Compilation (and therefore the grammar-availability
//! check) happens later, uniformly, for every built-in and installed pack.

use std::collections::BTreeMap;

use super::assets::{MANIFEST_FILE, PackAssets};
use super::error::PackError;
use super::extract::{ValidatedPack, compile_pack};
use super::manifest::{PackManifest, validate_manifest};

/// The Go starter pack's manifest and declared resources. Parsing a static,
/// crate-controlled manifest can only fail from a packaging mistake CI
/// would already catch (every test here, and `go_language_pack.rs`,
/// exercises this same construction), so this panics rather than threading
/// a `Result` through every caller for a condition that is never reachable
/// in a correctly built binary.
fn go_assets() -> PackAssets {
    let manifest_bytes: &[u8] = include_bytes!("../../packs/go/pack.json");
    let manifest: PackManifest =
        serde_json::from_slice(manifest_bytes).expect("built-in go pack.json parses");
    validate_manifest(&manifest).expect("built-in go pack.json validates");

    let mut files: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    files.insert(MANIFEST_FILE.to_string(), manifest_bytes.to_vec());
    files.insert(
        "declarations.scm".to_string(),
        include_bytes!("../../packs/go/declarations.scm").to_vec(),
    );
    files.insert(
        "calls.scm".to_string(),
        include_bytes!("../../packs/go/calls.scm").to_vec(),
    );
    files.insert(
        "imports.scm".to_string(),
        include_bytes!("../../packs/go/imports.scm").to_vec(),
    );
    files.insert(
        "adapter.rhai".to_string(),
        include_bytes!("../../packs/go/adapter.rhai").to_vec(),
    );
    files.insert(
        "rules.json".to_string(),
        include_bytes!("../../packs/go/rules.json").to_vec(),
    );
    files.insert(
        "toolchains.json".to_string(),
        include_bytes!("../../packs/go/toolchains.json").to_vec(),
    );
    files.insert(
        "fixtures/basic.go".to_string(),
        include_bytes!("../../packs/go/fixtures/basic.go").to_vec(),
    );
    files.insert(
        "fixtures/basic.expected.json".to_string(),
        include_bytes!("../../packs/go/fixtures/basic.expected.json").to_vec(),
    );

    PackAssets { manifest, files }
}

/// Raw manifest+asset bytes for every built-in pack. Independent of whether
/// capability-requiring features (the `rhai` feature, the `go-grammar`
/// feature) are compiled in — only manifest *shape* validation runs here,
/// never query compilation or script validation.
pub fn builtin_pack_sources() -> Vec<PackAssets> {
    vec![go_assets()]
}

/// Every built-in pack, compiled, for callers that want a ready-to-use
/// [`ValidatedPack`] without going through a project's `PackRegistry` — for
/// example, read-only graph inspection of a Go file in a project that never
/// ran `init` (the design's "Go's built-in assets ship in the crate and
/// support read-only graph inspection without `init`, like native
/// languages"). Returns `Err` for the first built-in pack that cannot be
/// compiled in this binary (e.g. Go without `go-grammar`); a caller that
/// wants built-ins tolerated as merely unsupported should go through
/// [`super::registry::inspect`] or [`super::registry::load`] instead, both
/// of which already handle that case for built-ins specifically.
pub fn builtin_packs() -> Result<Vec<ValidatedPack>, PackError> {
    builtin_pack_sources()
        .into_iter()
        .map(compile_pack)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn go_pack_sources_parse_and_validate_without_any_optional_feature() {
        let sources = builtin_pack_sources();
        assert_eq!(sources.len(), 1);
        let go = &sources[0];
        assert_eq!(go.manifest.id, "go");
        assert_eq!(go.manifest.grammar, "go");
        // Every declared resource path actually has bytes attached.
        for (field, path) in go.manifest.resource_paths() {
            assert!(
                go.files.contains_key(path),
                "declared resource {field} = {path} is missing from embedded assets"
            );
        }
    }

    #[test]
    #[cfg(feature = "go-grammar")]
    fn go_builtin_pack_compiles_when_the_grammar_feature_is_enabled() {
        let packs = builtin_packs().expect("go pack compiles with go-grammar");
        assert_eq!(packs.len(), 1);
    }

    #[test]
    #[cfg(not(feature = "go-grammar"))]
    fn go_builtin_pack_reports_unsupported_without_the_grammar_feature() {
        let err = builtin_packs().unwrap_err();
        assert_eq!(err.stage(), "unsupported", "{err}");
    }
}
