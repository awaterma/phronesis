// Pure cores of the coverage-store checks. This file is the ONE source of
// their executable code: `coverage/store.rs` and `coverage/region_map.rs`
// call it, and `verification/harness.rs` `include!`s it verbatim, so Verus
// proves exactly these function bodies. See the module doc on
// `mod pure_core` in `coverage/mod.rs` for the mechanism.
//
// Constraints that keep it compiling under both rustc and Verus:
// - plain `//` comments only (an `include!`d file cannot start with `//!`);
// - every Verus spec rides in `#[cfg_attr(verus_keep_ghost, ...)]` and every
//   proof step in a `#[cfg(verus_keep_ghost)] proof! { ... }` statement.
//   rustc never sets `verus_keep_ghost`, so it strips them before macro
//   resolution; Verus sets it, so it sees the specs. The spec functions they
//   name (`spec_identifier_byte`, `spec_fnv1a_64`, `spec_line_order`) are
//   defined in the harness, not here;
// - no iterators, closures, or std formatting: index `while` loops over
//   slices, which Verus verifies with the loop invariants below.

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

#[cfg(test)]
mod tests {
    use super::*;

    /// The pre-extraction production charset, kept as an independent oracle.
    fn legacy_char_ok(c: char) -> bool {
        matches!(c, 'A'..='Z' | 'a'..='z' | '0'..='9' | '_' | ':' | '.' | '/' | '-')
    }

    #[test]
    fn identifier_byte_ok_matches_the_legacy_charset_on_every_byte() {
        for b in 0..=u8::MAX {
            assert_eq!(
                identifier_byte_ok(b),
                legacy_char_ok(char::from(b)),
                "byte {b:#04x}"
            );
        }
    }

    #[test]
    fn identifier_bytes_ok_rejects_non_ascii_and_accepts_the_charset() {
        assert!(identifier_bytes_ok(b"fn:src/a.rs::f-1_x"));
        assert!(identifier_bytes_ok(b""));
        assert!(!identifier_bytes_ok("caf\u{e9}".as_bytes()));
        assert!(!identifier_bytes_ok(b"a b"));
        assert!(!identifier_bytes_ok(b"a\x00"));
    }

    #[test]
    fn fnv1a_64_matches_published_test_vectors() {
        assert_eq!(fnv1a_64(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a_64(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv1a_64(b"foobar"), 0x8594_4171_f739_67e8);
    }

    #[test]
    fn line_order_ok_is_start_le_end() {
        assert!(line_order_ok(1, 1));
        assert!(line_order_ok(1, 2));
        assert!(!line_order_ok(2, 1));
    }
}
