// DRAFT template — (rust, verus, determinism) for the coverage-store properties.
//
// Proves coverage_store.fnv1a_determinism against the shared production core.
//
// The rendered harness must be self-contained: the S5 body validator refuses
// `include!` and `#[path]`, so a rendered artifact cannot pull in
// `crates/phronesis-mcp/src/coverage/pure_core.rs` the way
// `verification/harness.rs` does. This template therefore embeds the
// block between `// BEGIN VERUS-SHARED CORE` and `// END VERUS-SHARED CORE`
// of that file VERBATIM, and the test
// `crates/phronesis-mcp/tests/verus_core_pin.rs` fails if the embedded copy
// and the production file ever differ. The proof is about the production
// bodies only while that test is green.
//
// Render contract: `property` (read-only map: id, subject, kind, condition,
// guarantee, depends_on, every value escaped by the host) and `facts`
// (frozen, sorted `property_depends_on` facts) are the whole scope. The
// script returns exactly one string. Property values may appear only in
// comments or string literals, so no property value is used as a name here.
// This is a draft: `phr-mcp verify` reads it only with `--allow-drafts`, and
// results from it never count as verified evidence.

let claim = if property.id == "coverage_store.fnv1a_determinism" {
    `
// The production FNV-1a 64 returns the same hash for the same input bytes.
fn claim_holds(a: &[u8], b: &[u8]) -> (same: bool)
    requires
        a@ == b@,
    ensures
        same,
{
    fnv1a_64(a) == fnv1a_64(b)
}
`
} else {
    throw `verus-determinism draft only models the coverage-store FNV-1a hash, not ${property.id}`;
};

let regions = "";
for fact in facts {
    // A double-quoted string: backtick strings do not process `\n`.
    regions += "// depends_on: " + fact.args[1] + "\n";
}

`// property: ${property.id} (${property.kind})
// condition: ${property.condition}
// guarantee: ${property.guarantee}
${regions}#![feature(proc_macro_hygiene)]
#![allow(unused_imports)]

use vstd::prelude::*;

verus! {

pub open spec fn spec_identifier_byte(b: u8) -> bool {
    (0x41 <= b <= 0x5a)
    || (0x61 <= b <= 0x7a)
    || (0x30 <= b <= 0x39)
    || b == 0x5f
    || b == 0x3a
    || b == 0x2e
    || b == 0x2f
    || b == 0x2d
}

pub open spec fn spec_fnv1a_64(data: Seq<u8>) -> u64
    decreases data.len()
{
    if data.len() == 0 {
        0xcbf29ce484222325u64
    } else {
        let prev = spec_fnv1a_64(data.take(data.len() - 1));
        (prev ^ (data[data.len() - 1] as u64)).wrapping_mul(0x100000001b3u64)
    }
}

pub open spec fn spec_line_order(start_line: u64, end_line: u64) -> bool {
    start_line <= end_line
}

} // verus!

// BEGIN VERUS-SHARED CORE
// (No backticks, backslashes, or dollar-brace between these markers: the
// block is embedded verbatim in a Rhai backtick string by the verus-*.rhai
// template drafts.)

/// A byte of a coverage identifier field (test, region, tool) is accepted iff
/// it is ASCII A-Z, a-z, 0-9, or one of _ : . / -
#[cfg_attr(verus_keep_ghost, verus_verify)]
#[cfg_attr(verus_keep_ghost, verus_spec(ok =>
    ensures ok == spec_identifier_byte(b)
))]
pub fn identifier_byte_ok(b: u8) -> bool {
    matches!(
        b,
        b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b':' | b'.' | b'/' | b'-'
    )
}

/// Every byte of data is an accepted identifier byte. Every accepted byte
/// is ASCII, so this rejects any non-ASCII UTF-8 sequence as well.
#[cfg_attr(verus_keep_ghost, verus_verify)]
#[cfg_attr(verus_keep_ghost, verus_spec(ok =>
    ensures ok == (forall|j: int| 0 <= j < data.len() ==> spec_identifier_byte(#[trigger] data[j]))
))]
pub fn identifier_bytes_ok(data: &[u8]) -> bool {
    let mut i: usize = 0;
    #[cfg_attr(verus_keep_ghost, verus_spec(
        invariant
            0 <= i <= data.len(),
            forall|j: int| 0 <= j < i ==> spec_identifier_byte(#[trigger] data[j]),
        decreases data.len() - i,
    ))]
    while i < data.len() {
        if !identifier_byte_ok(data[i]) {
            return false;
        }
        i += 1;
    }
    true
}

/// FNV-1a 64 over data: offset basis 0xcbf29ce484222325, prime
/// 0x100000001b3, xor-then-multiply per byte.
#[cfg_attr(verus_keep_ghost, verus_verify)]
#[cfg_attr(verus_keep_ghost, verus_spec(hash =>
    ensures hash == spec_fnv1a_64(data@)
))]
pub fn fnv1a_64(data: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut i: usize = 0;
    #[cfg_attr(verus_keep_ghost, verus_spec(
        invariant
            0 <= i <= data.len(),
            hash == spec_fnv1a_64(data@.take(i as int)),
        decreases data.len() - i,
    ))]
    while i < data.len() {
        hash ^= data[i] as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        i += 1;
        #[cfg(verus_keep_ghost)]
        proof! {
            assert(data@.take(i as int).take(i - 1) == data@.take(i - 1));
        }
    }
    #[cfg(verus_keep_ghost)]
    proof! {
        assert(data@.take(data.len() as int) == data@);
    }
    hash
}

/// A record's line span is well ordered: start_line <= end_line.
#[cfg_attr(verus_keep_ghost, verus_verify)]
#[cfg_attr(verus_keep_ghost, verus_spec(ok =>
    ensures ok == spec_line_order(start_line, end_line)
))]
pub fn line_order_ok(start_line: u64, end_line: u64) -> bool {
    start_line <= end_line
}

// END VERUS-SHARED CORE

verus! {
${claim}
fn main() {
}

} // verus!
`
