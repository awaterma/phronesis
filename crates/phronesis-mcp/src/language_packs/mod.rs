//! Starter language packs: a validated manifest, bounded assets, and the
//! typed records an adapter may produce.
//!
//! A pack directory holds `pack.json` plus the resources it declares. This
//! module owns the shared contract — schema, limits, path safety, digest, and
//! record validation — so individual adapters never re-implement them.

pub mod assets;
pub mod builtin;
pub mod cli;
pub mod context;
pub mod error;
pub mod extract;
pub mod facts;
pub mod fixtures;
pub mod grammar;
pub mod graph;
pub mod install;
pub mod integration;
pub mod manifest;
pub mod records;
pub mod registry;
pub mod selection;
pub mod templates;

pub use assets::{MANIFEST_FILE, PackAssets, digest_assets, read_assets};
pub use builtin::builtin_packs;
pub use context::file_context;
pub use error::PackError;
pub use extract::{ValidatedPack, compile_pack, extract};
pub use graph::{graph_records, resolve_pack_references};
pub use install::{InstallAction, InstallReport, ValidationReport, install, validate_directory};
pub use manifest::{
    BUNDLED_GRAMMARS, Capability, FixtureSpec, PackManifest, QuerySpec, RecordTemplate,
    check_ownership, validate_manifest,
};
pub use records::{
    Capture, ContextFile, Extraction, FileContext, PackDiagnostic, PackRecord, QueryMatch,
    RECORD_KINDS, RecordError, ReferenceMode, SourceSpan, TestCommand, validate_records,
};
pub use registry::{
    Activation, PackInfo, PackInventory, PackOrigin, PackRegistry, PackStatus, inspect, load,
};
pub use selection::{PackChangedSymbol, changed_pack_symbols, render_test_command};

/// Adapter limits. Exhausting one discards the extraction; it is never
/// truncated into successful evidence. Guard/provider limits are separate.
pub mod limits {
    const KIB: usize = 1024;
    const MIB: usize = 1024 * KIB;

    pub const MAX_INSTALLED_PACKS: usize = 64;
    pub const MAX_ASSETS: usize = 128;
    pub const MAX_MANIFEST_BYTES: usize = 64 * KIB;
    pub const MAX_SCRIPT_BYTES: usize = 64 * KIB;
    pub const MAX_QUERY_BYTES: usize = 64 * KIB;
    /// Source, context, fixture, rules, and toolchain files.
    pub const MAX_FILE_BYTES: usize = MIB;
    pub const MAX_TOTAL_ASSET_BYTES: usize = 8 * MIB;
    pub const MAX_QUERIES: usize = 32;
    pub const MAX_CONTEXT_FILES: usize = 4;
    pub const MAX_MATCHES: usize = 8_192;
    pub const MAX_CAPTURES: usize = 16_384;
    pub const MAX_CAPTURE_TEXT_BYTES: usize = 8 * MIB;
    pub const MAX_OUTPUT_RECORDS: usize = 4_096;
    pub const MAX_FACT_ARGS: usize = 32;
    pub const MAX_FACT_ARG_BYTES: usize = 4 * KIB;
    pub const PARSE_DEADLINE_MS: u64 = 250;
    pub const QUERY_DEADLINE_MS: u64 = 250;
    pub const RHAI_MAX_OPERATIONS: u64 = 2_000_000;
    pub const RHAI_MAX_CALL_DEPTH: usize = 16;
}

/// `[a-z][a-z0-9_]{0,47}` — pack IDs, language names, query IDs, fact names.
pub(crate) fn is_identifier(s: &str) -> bool {
    let mut bytes = s.bytes();
    matches!(bytes.next(), Some(b'a'..=b'z'))
        && s.len() <= 48
        && bytes.all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'_'))
}

/// Why `path` is not a normalized relative path, if it is not one: no
/// absolute, empty, `.`, or `..` components, backslashes, NUL bytes, or
/// colons. Colons are rejected on every platform: on Windows `C:/x`, `C:x`,
/// and a nested `a/C:/x` re-root a joined `PathBuf`, and `name:stream`
/// names an alternate data stream.
pub(crate) fn relative_path_problem(path: &str) -> Option<&'static str> {
    if path.is_empty() {
        Some("is empty")
    } else if path.starts_with('/') {
        Some("is absolute")
    } else if path.contains(':') {
        Some("contains a `:` (drive prefix or alternate data stream)")
    } else if path.contains('\\') || path.contains('\0') {
        Some("contains a backslash or NUL byte")
    } else if path.split('/').any(|c| c == "..") {
        Some("has a `..` component")
    } else if path.split('/').any(|c| c.is_empty() || c == ".") {
        Some("is not normalized (empty or `.` component)")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::relative_path_problem;

    #[test]
    fn relative_path_problem_rejects_windows_prefixes_and_streams() {
        for ok in ["a", "queries/decls.scm", "pkg/sub"] {
            assert_eq!(relative_path_problem(ok), None, "{ok}");
        }
        for bad in [
            "C:/outside.json",
            "C:outside.json",
            "c:",
            "queries/C:/outside.scm",
            "queries/D:outside.scm",
            "decls.scm:stream",
            "decls.scm::$DATA",
            "C:\\outside.json",
            "\\\\server\\share",
            "//server/share",
        ] {
            assert!(relative_path_problem(bad).is_some(), "{bad}");
        }
    }
}
