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
//
// and one property of toolchain outcome parsing:
//
//   6. graded_outcome — `CompiledDef::parse` and its count source in
//      `outcomes/toolchain.rs`, plus the `signal ingest` refusal in
//      `outcomes/mod.rs`: opting into `test_counts: "per_test"` never
//      invents a test verdict, never overrides a summary, and never turns a
//      compile failure or a failing test into a pass.

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
// Invariant 6: Graded toolchain outcome (mirror of `CompiledDef::parse`)
// ---------------------------------------------------------------------------
//
// The production rule in `outcomes/toolchain.rs`, for a non-proof def:
//
//     counts = summary_counts(output)            // first matching test_summary
//         .or_else(|| per_test_counts(output));  // only with test_counts: "per_test"
//     build  = if compile_fail_hit { Fail } else { match exit {
//         Some(0) => Pass, Some(_) if counts.is_some() => Pass, Some(_) => Fail,
//         None if counts.is_some() || compile_success_hit => Pass, None => Unknown } };
//     test   = (build == Pass).then(counts)      // test_outcome(passed, failed, total)
//     tag    = total > 0 && failed == 0 => test_pass; total > 0 => test_fail; else none
//
// and `record_from_output` refuses (`NoOutcome`) exactly when every tag is
// `compile_unknown`. Regex matching is abstracted to its results: whether a
// `compile_fail` / `compile_success` pattern hit, the summary's counts if one
// matched, and one bool per matched per-test line (`true` = a `pass_tokens`
// status).

pub enum Build {
    Pass,
    Fail,
    Unknown,
}

pub enum TestVerdict {
    NoVerdict,
    TestPass,
    TestFail,
}

/// Spec: how many per-test lines carry a pass status.
pub open spec fn spec_count_pass(lines: Seq<bool>) -> nat
    decreases lines.len(),
{
    if lines.len() == 0 {
        0
    } else {
        spec_count_pass(lines.drop_last()) + if lines.last() { 1nat } else { 0nat }
    }
}

/// Spec: `per_test_counts` — `None` unless opted in and at least one line
/// matched; then (passed, failed), but only when a line failed or the run
/// exited 0 (lines cannot vouch for tests that never reported).
pub open spec fn spec_per_test_counts(opt_in: bool, lines: Seq<bool>, exit: Option<i32>) -> Option<(nat, nat)> {
    if !opt_in || lines.len() == 0 {
        None
    } else if spec_count_pass(lines) < lines.len() || exit == Some(0i32) {
        Some((spec_count_pass(lines), (lines.len() - spec_count_pass(lines)) as nat))
    } else {
        None
    }
}

/// Spec: `test_counts` — a matching summary first, then the opt-in source.
pub open spec fn spec_counts(summary: Option<(nat, nat)>, opt_in: bool, lines: Seq<bool>, exit: Option<i32>) -> Option<(nat, nat)> {
    if summary.is_some() {
        summary
    } else {
        spec_per_test_counts(opt_in, lines, exit)
    }
}

/// Spec: the graded build table. `started`: counts exist, opted-in per-test
/// lines reported, or a `test_started` pattern hit (tests that started prove
/// the code compiled, even under a nonzero exit). Plain `compile_success`
/// text counts only when no exit code was captured.
pub open spec fn spec_build(
    compile_fail_hit: bool,
    exit: Option<i32>,
    reported: bool,
    started: bool,
    compile_success_hit: bool,
) -> Build {
    if compile_fail_hit {
        Build::Fail
    } else {
        match exit {
            Some(code) => if code == 0 || started { Build::Pass } else { Build::Fail },
            None => if reported || compile_success_hit { Build::Pass } else { Build::Unknown },
        }
    }
}

/// Spec: the test tag — emitted only after a passing build. A nonzero exit
/// vetoes a pass from any count source: with no failure counted, the run is
/// a run-level test failure (`test_run_failed`), which withdraws any earlier
/// pass.
pub open spec fn spec_test(build: Build, counts: Option<(nat, nat)>, exit: Option<i32>, tests_ran: bool) -> TestVerdict {
    let run_failed = match exit { Some(code) => code != 0, None => false };
    match (build, counts) {
        (Build::Pass, Some((passed, failed))) =>
            if run_failed && failed == 0 {
                TestVerdict::TestFail
            } else if passed + failed > 0 && failed == 0 {
                TestVerdict::TestPass
            } else if passed + failed > 0 {
                TestVerdict::TestFail
            } else {
                TestVerdict::NoVerdict
            },
        (Build::Pass, None) => if run_failed && tests_ran { TestVerdict::TestFail } else { TestVerdict::NoVerdict },
        _ => TestVerdict::NoVerdict,
    }
}

/// Spec: the whole outcome for one parsed run.
pub open spec fn spec_outcome(
    compile_fail_hit: bool,
    exit: Option<i32>,
    compile_success_hit: bool,
    test_started_hit: bool,
    summary: Option<(nat, nat)>,
    opt_in: bool,
    lines: Seq<bool>,
) -> (Build, TestVerdict) {
    let counts = spec_counts(summary, opt_in, lines, exit);
    let reported = counts.is_some() || (opt_in && lines.len() > 0);
    let started = reported || test_started_hit;
    let build = spec_build(compile_fail_hit, exit, reported, started, compile_success_hit);
    (build, spec_test(build, counts, exit, started))
}

/// Spec: `signal ingest` refuses exactly the runs whose only tag is
/// `compile_unknown`.
pub open spec fn spec_ingest_refused(outcome: (Build, TestVerdict)) -> bool {
    outcome.0 == Build::Unknown && outcome.1 == TestVerdict::NoVerdict
}

pub open spec fn lift(summary: Option<(u64, u64)>) -> Option<(nat, nat)> {
    match summary {
        Some((p, f)) => Some((p as nat, f as nat)),
        None => None,
    }
}

proof fn count_pass_bounded(lines: Seq<bool>)
    ensures spec_count_pass(lines) <= lines.len(),
    decreases lines.len(),
{
    if lines.len() > 0 {
        count_pass_bounded(lines.drop_last());
    }
}

/// Exec: count the passing per-test lines.
fn count_pass(lines: &[bool]) -> (n: u64)
    requires lines.len() <= u64::MAX,
    ensures n as nat == spec_count_pass(lines@),
{
    let mut n: u64 = 0;
    let mut i: usize = 0;
    while i < lines.len()
        invariant
            0 <= i <= lines.len(),
            lines.len() <= u64::MAX,
            n as nat == spec_count_pass(lines@.subrange(0, i as int)),
            n <= i,
        decreases lines.len() - i,
    {
        assert(lines@.subrange(0, i as int + 1).drop_last() =~= lines@.subrange(0, i as int));
        if lines[i] {
            n = n + 1;
        }
        i = i + 1;
    }
    assert(lines@.subrange(0, lines.len() as int) =~= lines@);
    n
}

/// Exec: the outcome, mirroring `CompiledDef::parse`. Verified to agree with
/// `spec_outcome` on every input.
fn outcome(
    compile_fail_hit: bool,
    exit: Option<i32>,
    compile_success_hit: bool,
    test_started_hit: bool,
    summary: Option<(u64, u64)>,
    opt_in: bool,
    lines: &[bool],
) -> (result: (Build, TestVerdict))
    requires lines.len() <= u64::MAX,
    ensures result == spec_outcome(compile_fail_hit, exit, compile_success_hit, test_started_hit, lift(summary), opt_in, lines@),
{
    let lines_seen = opt_in && lines.len() > 0;
    let counts: Option<(u64, u64)> = match summary {
        Some(c) => Some(c),
        None => if lines_seen {
            let passed = count_pass(lines);
            proof { count_pass_bounded(lines@); }
            let failed = lines.len() as u64 - passed;
            let clean_exit = match exit { Some(code) => code == 0, None => false };
            if failed > 0 || clean_exit { Some((passed, failed)) } else { None }
        } else {
            None
        },
    };
    let reported = counts.is_some() || lines_seen;
    let tests_ran = reported || test_started_hit;
    let build = if compile_fail_hit {
        Build::Fail
    } else {
        match exit {
            Some(code) => if code == 0 || tests_ran { Build::Pass } else { Build::Fail },
            None => if reported || compile_success_hit { Build::Pass } else { Build::Unknown },
        }
    };
    let run_failed = match exit { Some(code) => code != 0, None => false };
    let test = match (&build, counts) {
        (Build::Pass, Some((passed, failed))) =>
            if run_failed && failed == 0 {
                TestVerdict::TestFail
            } else if failed == 0 && passed > 0 {
                TestVerdict::TestPass
            } else if failed > 0 {
                TestVerdict::TestFail
            } else {
                TestVerdict::NoVerdict
            },
        (Build::Pass, None) => if run_failed && tests_ran { TestVerdict::TestFail } else { TestVerdict::NoVerdict },
        _ => TestVerdict::NoVerdict,
    };
    (build, test)
}

/// Proof (backward compatibility): without the opt-in, counts are exactly
/// the summary's — every def written before `test_counts` behaves as before.
proof fn opting_out_is_the_old_behaviour(summary: Option<(nat, nat)>, lines: Seq<bool>, exit: Option<i32>)
    ensures spec_counts(summary, false, lines, exit) == summary,
{
}

/// Proof (precedence): a matching summary always wins over per-test lines.
proof fn a_summary_wins(summary: Option<(nat, nat)>, opt_in: bool, lines: Seq<bool>, exit: Option<i32>)
    requires summary.is_some(),
    ensures spec_counts(summary, opt_in, lines, exit) == summary,
{
}

/// Proof (no invention): with no summary and no per-test line, there is no
/// test verdict, whatever the exit code says.
proof fn a_quiet_run_has_no_test_verdict(
    compile_fail_hit: bool, exit: Option<i32>, compile_success_hit: bool, test_started_hit: bool, opt_in: bool, lines: Seq<bool>,
)
    requires lines.len() == 0, !test_started_hit,
    ensures spec_outcome(compile_fail_hit, exit, compile_success_hit, test_started_hit, None, opt_in, lines).1 == TestVerdict::NoVerdict,
{
}

/// Proof (separation): a compile failure is a build failure with no test
/// verdict, so it is never confused with a failing test.
proof fn a_compile_failure_has_no_test_verdict(
    exit: Option<i32>, compile_success_hit: bool, test_started_hit: bool, summary: Option<(nat, nat)>, opt_in: bool, lines: Seq<bool>,
)
    ensures ({
        let o = spec_outcome(true, exit, compile_success_hit, test_started_hit, summary, opt_in, lines);
        o.0 == Build::Fail && o.1 == TestVerdict::NoVerdict
    }),
{
}

/// Proof (soundness of a pass): a test pass needs a passing build, at least
/// one passed test, and zero failures.
proof fn a_test_pass_is_grounded(
    compile_fail_hit: bool, exit: Option<i32>, compile_success_hit: bool, test_started_hit: bool,
    summary: Option<(nat, nat)>, opt_in: bool, lines: Seq<bool>,
)
    requires spec_outcome(compile_fail_hit, exit, compile_success_hit, test_started_hit, summary, opt_in, lines).1 == TestVerdict::TestPass,
    ensures ({
        let c = spec_counts(summary, opt_in, lines, exit);
        spec_outcome(compile_fail_hit, exit, compile_success_hit, test_started_hit, summary, opt_in, lines).0 == Build::Pass
            && c.is_some() && c.unwrap().0 > 0 && c.unwrap().1 == 0
    }),
{
}

proof fn a_failing_line_caps_the_count(lines: Seq<bool>, i: int)
    requires 0 <= i < lines.len(), !lines[i],
    ensures spec_count_pass(lines) < lines.len(),
    decreases lines.len(),
{
    count_pass_bounded(lines.drop_last());
    if i < lines.len() - 1 {
        a_failing_line_caps_the_count(lines.drop_last(), i);
    }
}

/// Proof (soundness of per-test counts): one failing per-test line rules out
/// a test pass when the counts come from per-test lines.
proof fn a_failing_line_rules_out_a_test_pass(
    compile_fail_hit: bool, exit: Option<i32>, compile_success_hit: bool, test_started_hit: bool, lines: Seq<bool>, i: int,
)
    requires 0 <= i < lines.len(), !lines[i],
    ensures spec_outcome(compile_fail_hit, exit, compile_success_hit, test_started_hit, None, true, lines).1 != TestVerdict::TestPass,
{
    a_failing_line_caps_the_count(lines, i);
}

/// Proof (a failed run is never a pass): with counts from per-test lines,
/// a nonzero exit never yields a test pass, even if every reported line
/// passed (a test that exited the process, or a timeout, reports nothing).
proof fn a_nonzero_exit_is_never_a_per_test_pass(
    compile_fail_hit: bool, code: i32, compile_success_hit: bool, test_started_hit: bool, lines: Seq<bool>,
)
    requires code != 0,
    ensures spec_outcome(compile_fail_hit, Some(code), compile_success_hit, test_started_hit, None, true, lines).1 != TestVerdict::TestPass,
{
    count_pass_bounded(lines);
}

/// Proof (the exit veto, for every count source): a nonzero exit never
/// yields a test pass, whether counts came from a summary or from per-test
/// lines. A clean summary from a run that died later is not a pass.
proof fn a_nonzero_exit_is_never_a_test_pass(
    compile_fail_hit: bool, code: i32, compile_success_hit: bool, test_started_hit: bool,
    summary: Option<(nat, nat)>, opt_in: bool, lines: Seq<bool>,
)
    requires code != 0,
    ensures spec_outcome(compile_fail_hit, Some(code), compile_success_hit, test_started_hit, summary, opt_in, lines).1 != TestVerdict::TestPass,
{
}

/// Proof (withdrawal): when tests ran and the run exited nonzero, the
/// verdict is a test failure, never silence, so it supersedes an earlier
/// pass for the subject.
proof fn a_failed_run_where_tests_ran_is_a_test_failure(
    code: i32, compile_success_hit: bool, test_started_hit: bool, summary: Option<(nat, nat)>, opt_in: bool, lines: Seq<bool>,
)
    requires
        code != 0,
        summary.is_some() || (opt_in && lines.len() > 0) || test_started_hit,
    ensures spec_outcome(false, Some(code), compile_success_hit, test_started_hit, summary, opt_in, lines).1 == TestVerdict::TestFail,
{
    count_pass_bounded(lines);
}

/// Proof (truncation is never a pass): with no exit code, per-test lines
/// can ground a failure but never a test pass.
proof fn no_exit_is_never_a_per_test_pass(
    compile_fail_hit: bool, compile_success_hit: bool, test_started_hit: bool, lines: Seq<bool>,
)
    ensures spec_outcome(compile_fail_hit, None, compile_success_hit, test_started_hit, None, true, lines).1 != TestVerdict::TestPass,
{
    count_pass_bounded(lines);
}

/// Proof (absent evidence is unknown): no exit code, no compile pattern hit
/// and no counts is `unknown`, never a pass.
proof fn no_exit_and_no_evidence_is_unknown(opt_in: bool, lines: Seq<bool>)
    requires !opt_in || lines.len() == 0,
    ensures spec_outcome(false, None, false, false, None, opt_in, lines).0 == Build::Unknown,
{
}

/// Proof (ingest): `signal ingest` refuses exactly the runs with an unknown
/// build — a compile failure, a compile pass, and any test verdict are all
/// recorded.
proof fn ingest_refuses_exactly_the_ungrounded(
    compile_fail_hit: bool, exit: Option<i32>, compile_success_hit: bool, test_started_hit: bool,
    summary: Option<(nat, nat)>, opt_in: bool, lines: Seq<bool>,
)
    ensures ({
        let o = spec_outcome(compile_fail_hit, exit, compile_success_hit, test_started_hit, summary, opt_in, lines);
        spec_ingest_refused(o) <==> o.0 == Build::Unknown
    }),
{
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

    // Smoke-test: graded outcome (`go test -v` shapes)
    let pass_lines = [true, true];
    let fail_lines = [true, false];
    let quiet: [bool; 0] = [];
    let passing = outcome(false, Some(0), false, false, None, true, &pass_lines);
    let failing = outcome(false, Some(1), false, false, None, true, &fail_lines);
    let exited = outcome(false, Some(1), false, false, None, true, &pass_lines);
    let compile_error = outcome(true, Some(1), false, false, None, true, &quiet);
    let quiet_run = outcome(false, Some(0), true, false, None, true, &quiet);
    let summary_then_crash = outcome(false, Some(1), false, false, Some((3, 0)), false, &quiet);
    let died_first = outcome(false, Some(1), false, true, None, true, &quiet);
    let _ = (passing, failing, exited, compile_error, quiet_run, summary_then_crash, died_first);
}

} // verus!