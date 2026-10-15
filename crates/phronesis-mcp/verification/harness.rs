// verification/harness.rs
//
// Verus-verified harness for coverage-store invariants.
//
// This module formally verifies three properties of the coverage-evidence
// code in `crates/phronesis-mcp/src/coverage/`:
//
//   1. valid_identifier_char — mirrors the charset rule from
//      `validate_identifier_field` in `coverage/store.rs`:
//      A-Za-z0-9 and `_ : . / -` are the only allowed characters.
//
//   2. fnv1a_64_determinism — the FNV-1a hash in `coverage/region_map.rs`
//      is a pure function: same input bytes always produce the same output.
//
//   3. line_ordering — `validate_record` requires start_line <= end_line;
//      we verify that a simple checker enforces this.
//
// and one property of the code graph's query surface:
//
//   4. boundary_suffix_match — the bare-name arm of `arg_matches` in
//      `graph/query.rs` matches only at a `:` boundary, finds every ID of
//      the form `<prefix ending in ':'><name>`, and narrows as a name is
//      qualified.

#![allow(unused_imports)]

use vstd::prelude::*;
use vstd::seq::*;

verus! {

// ---------------------------------------------------------------------------
// Invariant 1: Identifier charset (mirror of validate_identifier_field)
// ---------------------------------------------------------------------------

/// Spec: characterise the set of bytes that `validate_identifier_field`
/// accepts. This is the charset `A-Za-z0-9_:. /-`.
spec fn is_valid_identifier_byte(b: u8) -> bool {
    (b >= 0x41 && b <= 0x5a)   // A-Z
    || (b >= 0x61 && b <= 0x7a) // a-z
    || (b >= 0x30 && b <= 0x39) // 0-9
    || b == 0x5f               // _
    || b == 0x3a               // :
    || b == 0x2e               // .
    || b == 0x2f               // /
    || b == 0x2d               // -
}

/// Exec function: runtime check that mirrors the spec.
/// Verified to agree with `is_valid_identifier_byte` for every byte.
fn valid_identifier_char(b: u8) -> (result: bool)
    ensures result == is_valid_identifier_byte(b)
{
    if b >= 0x41 && b <= 0x5a {
        true
    } else if b >= 0x61 && b <= 0x7a {
        true
    } else if b >= 0x30 && b <= 0x39 {
        true
    } else if b == 0x5f {
        true
    } else if b == 0x3a {
        true
    } else if b == 0x2e {
        true
    } else if b == 0x2f {
        true
    } else if b == 0x2d {
        true
    } else {
        false
    }
}

/// Exec function: check that every byte in a slice is a valid identifier byte.
fn valid_identifier_slice(data: &[u8]) -> (result: bool)
    ensures result == forall|i: int| 0 <= i < data.len() ==> is_valid_identifier_byte(#[trigger] data[i])
{
    let mut i: usize = 0;
    while i < data.len()
        invariant
            forall|j: int| 0 <= j < i ==> is_valid_identifier_byte(#[trigger] data[j]),
        decreases data.len() - i,
    {
        if !valid_identifier_char(data[i]) {
            return false;
        }
        i = i + 1;
    }
    true
}

// ---------------------------------------------------------------------------
// Invariant 2: FNV-1a 64-bit hash determinism
// ---------------------------------------------------------------------------

/// Spec function: FNV-1a over a seq of bytes, defined recursively on length.
/// Uses take() (which is open) rather than subrange() (which is closed).
pub open spec fn spec_fnv1a_64(data: Seq<u8>) -> u64
    decreases data.len()
{
    if data.len() == 0 {
        0xcbf29ce484222325u64
    } else {
        let prev = spec_fnv1a_64(data.take(data.len() - 1));
        let byte = data[data.len() - 1];
        (prev ^ (byte as u64)).wrapping_mul(0x100000001b3u64)
    }
}

/// Exec function: FNV-1a 64-bit hash, mirroring `fnv1a_64` in region_map.rs.
/// Verified to equal spec_fnv1a_64 for all inputs.
fn fnv1a_64(data: &[u8]) -> (hash: u64)
    ensures hash == spec_fnv1a_64(data@)
{
    let mut hash: u64 = 0xcbf29ce484222325u64;
    let mut i: usize = 0;
    while i < data.len()
        invariant
            0 <= i <= data.len(),
            hash == spec_fnv1a_64(data@.take(i as int)),
        decreases data.len() - i,
    {
        let byte = data[i] as u64;
        hash = (hash ^ byte).wrapping_mul(0x100000001b3u64);
        i = i + 1;
        // After increment, hash should equal spec_fnv1a_64(data@.take(i))
        // The recurrence: spec_fnv1a_64(data@.take(i)) =
        //   (spec_fnv1a_64(data@.take(i-1)) ^ data@[i-1]) * prime
        // This follows from take(i) == take(i-1).push(data@[i-1])
        // and the spec_fnv1a_64 recurrence on non-empty seqs.
        assert(data@.take(i as int).len() == i);
        assert(data@.take(i as int)[i - 1] == data[i - 1]);
        assert(data@.take(i as int).take((i - 1) as int) == data@.take((i - 1) as int));
    }
    // When loop exits, i == data.len(), so take(i) == data@
    assert(data@.take(data.len() as int) == data@);
    hash
}

/// Proof: FNV-1a is deterministic — same input always produces same output.
/// Since fnv1a_64 ensures hash == spec_fnv1a_64(data@), and spec_fnv1a_64
/// is a pure spec function, equal inputs yield equal outputs.
proof fn fnv1a_64_determinism(data1: &[u8], data2: &[u8])
    requires data1@ == data2@
    ensures spec_fnv1a_64(data1@) == spec_fnv1a_64(data2@)
{
    // The spec function is deterministic by construction (pure mathematical function).
    // Since data1@ == data2@, their spec_fnv1a_64 values are equal.
}

// ---------------------------------------------------------------------------
// Invariant 3: Line ordering (start_line <= end_line)
// ---------------------------------------------------------------------------

/// Spec function: line range is valid iff start <= end.
spec fn spec_valid_line_range(start_line: u64, end_line: u64) -> bool {
    start_line <= end_line
}

/// Exec function: verify start_line <= end_line, mirroring the check in
/// `validate_record`.
fn valid_line_range(start_line: u64, end_line: u64) -> (result: bool)
    ensures result == spec_valid_line_range(start_line, end_line)
{
    start_line <= end_line
}

/// Proof: valid_line_range agrees with spec_valid_line_range.
proof fn valid_line_range_correct(start: u64, end: u64)
    ensures spec_valid_line_range(start, end) == (start <= end)
{
    // Trivially true by definition of spec_valid_line_range.
}

// ---------------------------------------------------------------------------
// Invariant 4: Bare-name graph query matching (mirror of `arg_matches`)
// ---------------------------------------------------------------------------
//
// `graph query` / `graph ownership` let a literal argument (no `*` / `?`)
// match a fully qualified node ID by suffix, but only when the suffix starts
// right after a `:` — so `fire_all_consequences` finds
// `rust:demo::Net::fire_all_consequences` while `consequences` does not. The
// production rule in `graph/query.rs` is:
//
//     got.strip_suffix(want)
//         .is_some_and(|prefix| want.starts_with(':') || prefix.ends_with(':'))
//
// Only this suffix arm is modelled; the glob arm (`glob_matches`) is not.

/// Spec: `got` ends with `want`.
pub open spec fn spec_ends_with(got: Seq<u8>, want: Seq<u8>) -> bool {
    want.len() <= got.len()
        && forall|i: int| 0 <= i < want.len() ==> #[trigger] got[got.len() - want.len() + i] == want[i]
}

/// Spec: the suffix arm of `arg_matches`. `prefix` is `got` minus `want`, so
/// `prefix.ends_with(':')` is the byte just before the suffix.
pub open spec fn spec_boundary_suffix_match(want: Seq<u8>, got: Seq<u8>) -> bool {
    spec_ends_with(got, want)
        && ((want.len() > 0 && want[0] == 0x3a)
            || (got.len() > want.len() && got[got.len() - want.len() - 1] == 0x3a))
}

/// Exec function: the suffix arm of `arg_matches` over bytes.
/// Verified to agree with `spec_boundary_suffix_match` on every input.
fn boundary_suffix_match(want: &[u8], got: &[u8]) -> (result: bool)
    ensures result == spec_boundary_suffix_match(want@, got@)
{
    if want.len() > got.len() {
        return false;
    }
    let start = got.len() - want.len();
    let mut i: usize = 0;
    while i < want.len()
        invariant
            want.len() <= got.len(),
            start == got.len() - want.len(),
            0 <= i <= want.len(),
            forall|j: int| 0 <= j < i ==> #[trigger] got@[got@.len() - want@.len() + j] == want@[j],
        decreases want.len() - i,
    {
        if got[start + i] != want[i] {
            return false;
        }
        i = i + 1;
    }
    (want.len() > 0 && want[0] == 0x3a) || (start > 0 && got[start - 1] == 0x3a)
}

/// Proof (soundness): a bare name that does not begin with `:` only ever
/// matches a suffix that starts right after a `:` — it never matches from
/// the middle of a segment, so `consequences` cannot match
/// `fire_all_consequences`.
proof fn boundary_match_never_splits_a_segment(want: Seq<u8>, got: Seq<u8>)
    requires
        spec_boundary_suffix_match(want, got),
        want.len() == 0 || want[0] != 0x3a,
    ensures
        got.len() > want.len(),
        got[got.len() - want.len() - 1] == 0x3a,
{
}

/// Proof (characterisation): for a name not beginning with `:`, matching is
/// exactly "the ID is some prefix ending in `:` followed by the name".
/// Every such ID is found (completeness) and nothing else is (soundness).
proof fn boundary_match_iff_colon_prefix(want: Seq<u8>, got: Seq<u8>)
    requires
        want.len() == 0 || want[0] != 0x3a,
    ensures
        spec_boundary_suffix_match(want, got) <==> (exists|p: Seq<u8>|
            p.len() > 0 && p[p.len() - 1] == 0x3a && #[trigger] (p + want) =~= got),
{
    if spec_boundary_suffix_match(want, got) {
        let p = got.take(got.len() - want.len());
        assert forall|i: int| 0 <= i < got.len() implies #[trigger] (p + want)[i] == got[i] by {
            if i >= p.len() {
                assert(got[got.len() - want.len() + (i - p.len())] == want[i - p.len()]);
            }
        }
        assert((p + want) =~= got);
    }
    if exists|p: Seq<u8>| p.len() > 0 && p[p.len() - 1] == 0x3a && #[trigger] (p + want) =~= got {
        let p = choose|p: Seq<u8>| p.len() > 0 && p[p.len() - 1] == 0x3a && #[trigger] (p + want) =~= got;
        assert forall|i: int| 0 <= i < want.len() implies #[trigger] got[got.len() - want.len() + i] == want[i] by {
            assert(got[p.len() + i] == (p + want)[p.len() + i]);
        }
        assert(got[got.len() - want.len() - 1] == (p + want)[p.len() - 1]);
    }
}

/// Proof (monotonicity): qualifying a name never widens its matches. If
/// `Net::fire_all_consequences` matches an ID, so does
/// `fire_all_consequences` — adding segments can only narrow a query.
proof fn qualifying_a_name_only_narrows(q: Seq<u8>, want: Seq<u8>, got: Seq<u8>)
    requires
        q.len() > 0,
        q[q.len() - 1] == 0x3a,
        spec_boundary_suffix_match(q + want, got),
    ensures
        spec_boundary_suffix_match(want, got),
{
    let qw = q + want;
    assert forall|i: int| 0 <= i < want.len() implies #[trigger] got[got.len() - want.len() + i] == want[i] by {
        assert(got[got.len() - qw.len() + (q.len() + i)] == qw[q.len() + i]);
    }
    assert(got[got.len() - qw.len() + (q.len() - 1)] == qw[q.len() - 1]);
}

// ---------------------------------------------------------------------------
// Entry point (exec, for standalone execution)
// ---------------------------------------------------------------------------

fn main() {
    // Smoke-test: valid identifier bytes
    let valid_a = valid_identifier_char(b'A');
    assert(valid_a);
    let valid_z = valid_identifier_char(b'z');
    assert(valid_z);
    let valid_0 = valid_identifier_char(b'0');
    assert(valid_0);
    let valid_under = valid_identifier_char(b'_');
    assert(valid_under);
    let valid_colon = valid_identifier_char(b':');
    assert(valid_colon);
    let valid_dot = valid_identifier_char(b'.');
    assert(valid_dot);
    let valid_slash = valid_identifier_char(b'/');
    assert(valid_slash);
    let valid_dash = valid_identifier_char(b'-');
    assert(valid_dash);
    let invalid_space = valid_identifier_char(b' ');
    assert(!invalid_space);
    let invalid_null = valid_identifier_char(0x00);
    assert(!invalid_null);

    // Smoke-test: FNV-1a determinism
    let h1 = fnv1a_64(b"hello world");
    let h2 = fnv1a_64(b"hello world");
    assert(h1 == h2);

    // Smoke-test: line ordering
    let r1 = valid_line_range(1, 10);
    assert(r1);
    let r2 = valid_line_range(5, 5);
    assert(r2);
    let r3 = valid_line_range(10, 1);
    assert(!r3);

    // Smoke-test: bare-name matching
    let hit = boundary_suffix_match(b"acquire", b"rust:demo::Scheduler::acquire");
    let qualified = boundary_suffix_match(b"Scheduler::acquire", b"rust:demo::Scheduler::acquire");
    let leading = boundary_suffix_match(b"::acquire", b"rust:demo::Scheduler::acquire");
    let split = boundary_suffix_match(b"quire", b"rust:demo::Scheduler::acquire");
    let exact = boundary_suffix_match(b"rust:demo::wme", b"rust:demo::wme");
    // Byte-string literals are opaque to the verifier here, so these run
    // rather than assert; the general cases are the proofs above, and the
    // concrete ones are pinned against production `arg_matches` by
    // `literal_matching_agrees_with_the_verified_spec_exhaustively`.
    let _ = (hit, qualified, leading, split, exact);
}

} // verus!