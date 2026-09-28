//! Pins every Verus copy of the coverage-store core to the production file.
//!
//! `src/coverage/pure_core.rs` is the one executable source. The checked-in
//! harness `include!`s it, so it cannot drift. Rendered artifacts cannot
//! (the S5 validator refuses `include!` and `#[path]`), so the
//! `verus-<kind>.rhai` template drafts embed the marked core block verbatim.
//! These tests fail the moment that embedded copy differs from production,
//! so a proof rendered from a draft is always about the production bodies.

use std::path::{Path, PathBuf};

const BEGIN: &str = "// BEGIN VERUS-SHARED CORE";
const END: &str = "// END VERUS-SHARED CORE";

fn crate_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// The marked block of `pure_core.rs`, markers included.
fn core_block() -> String {
    let src = read(&crate_dir().join("src/coverage/pure_core.rs"));
    let start = src.find(BEGIN).expect("BEGIN marker in pure_core.rs");
    let end = src.find(END).expect("END marker in pure_core.rs") + END.len();
    assert!(start < end, "markers out of order in pure_core.rs");
    src[start..end].to_string()
}

#[test]
fn core_block_is_embeddable_in_a_rhai_backtick_string() {
    let block = core_block();
    for bad in ["`", "${", "\\"] {
        assert!(
            !block.contains(bad),
            "pure_core.rs core block contains {bad:?}, which a Rhai backtick string cannot carry verbatim"
        );
    }
    for f in [
        "identifier_byte_ok",
        "identifier_bytes_ok",
        "fnv1a_64",
        "line_order_ok",
    ] {
        assert!(
            block.contains(&format!("pub fn {f}(")),
            "{f} is outside the core block"
        );
    }
}

#[test]
fn checked_in_harness_includes_the_production_core() {
    let harness = read(&crate_dir().join("verification/harness.rs"));
    assert!(
        harness.contains(r#"include!("../src/coverage/pure_core.rs");"#),
        "verification/harness.rs must include! the production core, not a copy"
    );
    for f in [
        "fn identifier_byte_ok(",
        "fn fnv1a_64(",
        "fn line_order_ok(",
    ] {
        assert!(
            !harness.contains(f),
            "verification/harness.rs defines {f}: that is a mirror of the production core"
        );
    }
}

#[test]
fn template_drafts_embed_the_production_core_verbatim() {
    let block = core_block();
    let drafts = crate_dir().join("../../verification/template-drafts");
    for name in ["verus-invariant.rhai", "verus-determinism.rhai"] {
        let template = read(&drafts.join(name));
        assert!(
            template.contains(&block),
            "{name} does not embed the current pure_core.rs block verbatim; \
             re-copy the block between {BEGIN:?} and {END:?}"
        );
    }
}
