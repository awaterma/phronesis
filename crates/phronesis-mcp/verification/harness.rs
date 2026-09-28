// verification/harness.rs
//
// Verus harness for the coverage-store invariants. It proves the PRODUCTION
// code: the executable bodies come from `src/coverage/pure_core.rs` via the
// `include!` below, the same file `coverage/store.rs` and
// `coverage/region_map.rs` call. There is no copy of those bodies here.
//
// Mechanism: `pure_core.rs` carries its `ensures` clauses and loop
// invariants as `#[cfg_attr(verus_keep_ghost, verus_spec(...))]` (Verus's
// attribute form, which leaves the function in plain Rust syntax) and its
// proof steps as `#[cfg(verus_keep_ghost)] proof! { ... }`. Verus sets
// `verus_keep_ghost`, so it verifies them; rustc does not, so it strips them.
// This harness supplies what those attributes name: the spec functions
// below, and the crate-level `proc_macro_hygiene` feature that lets the
// loop-level `verus_spec` attributes expand.
//
// Properties (ids in .phronesis/properties.json):
//
//   coverage_store.valid_identifier_charset
//     identifier_byte_ok(b)       == spec_identifier_byte(b)       for every u8
//     identifier_bytes_ok(data)   == forall i. spec_identifier_byte(data[i])
//   coverage_store.fnv1a_determinism
//     fnv1a_64(data)              == spec_fnv1a_64(data@)          (total spec
//     function, so equal byte sequences hash equal: fnv1a_64_determinism)
//   coverage_store.line_ordering
//     line_order_ok(start, end)   == (start <= end)

#![feature(proc_macro_hygiene)]
#![allow(unused_imports)]

use vstd::prelude::*;

verus! {

/// The identifier charset `A-Za-z0-9` and `_ : . / -`, stated over byte
/// values so the spec is independent of the byte-literal code it checks.
pub open spec fn spec_identifier_byte(b: u8) -> bool {
    (0x41 <= b <= 0x5a)        // A-Z
    || (0x61 <= b <= 0x7a)     // a-z
    || (0x30 <= b <= 0x39)     // 0-9
    || b == 0x5f               // _
    || b == 0x3a               // :
    || b == 0x2e               // .
    || b == 0x2f               // /
    || b == 0x2d               // -
}

/// FNV-1a 64 as a recursive definition on the byte sequence: offset basis
/// 0xcbf29ce484222325, prime 0x100000001b3 (the published parameters).
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

/// A line span is well ordered.
pub open spec fn spec_line_order(start_line: u64, end_line: u64) -> bool {
    start_line <= end_line
}

/// Determinism: the production hash equals a total spec function of the
/// input bytes, so equal byte sequences always hash to the same value.
proof fn fnv1a_64_determinism(a: Seq<u8>, b: Seq<u8>)
    requires a == b
    ensures spec_fnv1a_64(a) == spec_fnv1a_64(b)
{
}

} // verus!

// The production cores, verbatim. Their specs are attached in the file.
include!("../src/coverage/pure_core.rs");

verus! {

fn main() {
    // Smoke checks through the production functions' ensures clauses.
    let ok_a = identifier_byte_ok(b'A');
    assert(ok_a);
    let ok_dash = identifier_byte_ok(b'-');
    assert(ok_dash);
    let bad_space = identifier_byte_ok(b' ');
    assert(!bad_space);
    let bad_nul = identifier_byte_ok(0x00);
    assert(!bad_nul);

    let h1 = fnv1a_64(b"hello world");
    let h2 = fnv1a_64(b"hello world");
    assert(h1 == h2);

    let r1 = line_order_ok(1, 10);
    assert(r1);
    let r2 = line_order_ok(5, 5);
    assert(r2);
    let r3 = line_order_ok(10, 1);
    assert(!r3);
}

} // verus!
