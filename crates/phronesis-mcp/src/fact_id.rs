//! Collision-free ids for facts the hook derives.
//!
//! The engine treats a second fact with an existing id but different content
//! as an error, and the hook fails closed on that error. Ids used to be built
//! by `_`-joining sanitized fragments, so `new_content_contains("a.b")` and
//! `new_content_contains("a-b")` both became `new_content_contains_a_b`, and
//! two *warn* rules produced a spurious block. The same scheme let
//! `function_clone_count` for fn `high_x` collide with
//! `function_clone_count_high` for fn `x`.
//!
//! [`fact_id`] is injective: the predicate and each part are escaped
//! reversibly and joined with `:`, which escaping never emits. Identical
//! inputs still produce identical ids, so two rules naming the same pattern
//! share one fact (the engine treats an identical re-assertion as a no-op).

/// Bytes kept verbatim. Everything else — including the `%` escape and the
/// `:` separator — becomes `%XX`.
fn is_plain(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b'/')
}

fn escape_into(out: &mut String, s: &str) {
    for &b in s.as_bytes() {
        if is_plain(b) {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
}

/// `predicate:part1:part2…`, each component escaped so distinct
/// `(predicate, parts)` inputs never share an id.
pub(crate) fn fact_id(predicate: &str, parts: &[&str]) -> String {
    let mut out =
        String::with_capacity(predicate.len() + parts.iter().map(|p| p.len() + 1).sum::<usize>());
    escape_into(&mut out, predicate);
    for part in parts {
        out.push(':');
        escape_into(&mut out, part);
    }
    out
}

/// Assign every fact in `facts` the id
/// `fact_id(predicate, args…, occurrence)`, where `occurrence` counts earlier
/// facts with the same predicate and args. Repeats stay separate facts (as
/// they were under the old per-kind index), and nothing in the batch can
/// collide with anything else.
pub(crate) fn assign_ids(facts: &mut [phr::Fact]) {
    let mut occurrences: std::collections::HashMap<(String, Vec<String>), usize> =
        std::collections::HashMap::new();
    for fact in facts {
        let n = occurrences
            .entry((fact.predicate.clone(), fact.args.clone()))
            .or_insert(0);
        let index = n.to_string();
        let mut parts: Vec<&str> = fact.args.iter().map(String::as_str).collect();
        parts.push(&index);
        fact.id = fact_id(&fact.predicate, &parts);
        *n += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// Inverse of [`fact_id`]; exists to prove the encoding is injective.
    fn decode(id: &str) -> (String, Vec<String>) {
        let unescape = |s: &str| {
            let bytes = s.as_bytes();
            let mut out = Vec::new();
            let mut i = 0;
            while i < bytes.len() {
                if bytes[i] == b'%' {
                    out.push(u8::from_str_radix(&s[i + 1..i + 3], 16).expect("hex escape"));
                    i += 3;
                } else {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
            String::from_utf8(out).expect("utf-8")
        };
        let mut components = id.split(':').map(unescape);
        let predicate = components.next().unwrap_or_default();
        (predicate, components.collect())
    }

    #[test]
    fn regression_punctuation_twins_get_distinct_ids() {
        assert_ne!(
            fact_id("new_content_contains", &["a.b"]),
            fact_id("new_content_contains", &["a-b"])
        );
        assert_ne!(
            fact_id("new_content_contains", &["a b"]),
            fact_id("new_content_contains", &["a_b"])
        );
        // Cross-predicate: the old `_` join made these the same string.
        assert_ne!(
            fact_id("function_clone_count", &["high_x", "0"]),
            fact_id("function_clone_count_high", &["x", "0"])
        );
        assert_ne!(fact_id("p", &["a:b"]), fact_id("p", &["a", "b"]));
        assert_ne!(fact_id("p", &["a%3Ab"]), fact_id("p", &["a:b"]));
    }

    #[test]
    fn assign_ids_keeps_repeats_distinct() {
        let fact = |predicate: &str, args: &[&str]| phr::Fact {
            id: String::new(),
            predicate: predicate.to_string(),
            args: args.iter().map(|a| a.to_string()).collect(),
            timestamp: 0,
            source: None,
        };
        let mut facts = vec![
            fact("function_clone_count", &["f.rs", "high_x", "3"]),
            fact("function_clone_count_high", &["f.rs", "x", "3"]),
            fact("function_clone_count", &["f.rs", "high_x", "3"]),
        ];
        assign_ids(&mut facts);
        let ids: std::collections::HashSet<&str> = facts.iter().map(|f| f.id.as_str()).collect();
        assert_eq!(ids.len(), 3, "{facts:?}");
    }

    #[test]
    fn identical_inputs_share_one_id() {
        assert_eq!(
            fact_id("new_content_contains", &["\\.unwrap\\(\\)"]),
            fact_id("new_content_contains", &["\\.unwrap\\(\\)"])
        );
    }

    /// Property: `decode(fact_id(x)) == x` for every generated `x`, which
    /// makes `fact_id` injective — distinct inputs, distinct ids. Exhaustive
    /// over short strings of an adversarial alphabet (separator, escape,
    /// punctuation the old scheme folded to `_`, multi-byte UTF-8), plus a
    /// deterministic pseudo-random sweep over longer inputs.
    #[test]
    fn property_distinct_inputs_yield_distinct_ids() {
        const ALPHABET: [&str; 9] = ["a", "_", ".", "-", ":", "%", " ", "é", "3A"];
        let mut strings: Vec<String> = vec![String::new()];
        for _ in 0..3 {
            let next: Vec<String> = strings
                .iter()
                .flat_map(|s| ALPHABET.iter().map(move |c| format!("{s}{c}")))
                .collect();
            strings.extend(next);
        }
        strings.sort();
        strings.dedup();

        let mut seen: HashMap<String, (String, Vec<String>)> = HashMap::new();
        let mut check = |predicate: &str, parts: &[&str]| {
            let id = fact_id(predicate, parts);
            let input = (
                predicate.to_string(),
                parts.iter().map(|p| p.to_string()).collect::<Vec<_>>(),
            );
            assert_eq!(decode(&id), input, "round-trip failed for {id}");
            if let Some(prev) = seen.insert(id.clone(), input.clone()) {
                assert_eq!(prev, input, "collision on {id}");
            }
        };
        for predicate in ["p", "p_high", "p:q"] {
            for s in &strings {
                check(predicate, &[s]);
            }
        }
        for a in strings.iter().take(200) {
            for b in strings.iter().take(200) {
                check("p", &[a, b]);
            }
        }

        // xorshift sweep: longer strings, 1–4 parts.
        let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        for _ in 0..20_000 {
            let n_parts = 1 + (next() % 4) as usize;
            let parts: Vec<String> = (0..n_parts)
                .map(|_| {
                    let len = (next() % 12) as usize;
                    (0..len)
                        .map(|_| ALPHABET[(next() % ALPHABET.len() as u64) as usize])
                        .collect()
                })
                .collect();
            let refs: Vec<&str> = parts.iter().map(String::as_str).collect();
            check("new_content_contains", &refs);
        }
    }
}
