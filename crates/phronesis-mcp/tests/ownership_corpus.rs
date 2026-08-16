//! The ownership acceptance corpus, executed as a regression suite
//! (SPEC-rust-ownership-evidence §12/§13.1, decisions D2/D5/D6/D7/D12/D19/D20/D21/D22).
//!
//! `tests/fixtures/ownership/` holds 23 minimized Rust files derived from the
//! five documented real-world case shapes plus §12's adversarial list, and a
//! `MANIFEST.md` stating per file what a correct extractor must produce and what
//! it must not. The corpus is only worth what a test asserts about it: until
//! this file existed a fixture could have been emptied, deleted, or contradicted
//! without a single failure, and the manifest could say anything at all.
//!
//! Three layers of assertion live here.
//!
//! **Structural sweep.** Run the extractor over every fixture and check the
//! invariants that must hold for all of them at once: base edges only, `src`
//! equal to the fixture's own path (D12), the documented arity and argument
//! order of every relation (§6.1/§6.2 — nothing in this codebase validates
//! arity, so a typo produces a pattern that silently never matches, D18), spans
//! that address real bytes of the file they name, and operands that obey D7.
//!
//! **Closure.** D19 in its literal form: the union of relation names emitted
//! across the whole corpus is a subset of `AST_EMITTABLE`, and the compiler-only
//! relations never appear. This is the test that actually satisfies §13.1 —
//! a comment beside the derivation code does not. D21's function-id set
//! equality sits here too.
//!
//! **Per-case expectations from the MANIFEST.** For each high-value fixture,
//! both the required output *and* the forbidden conclusion, because a negative
//! result that came from the extractor seeing nothing is worthless.
//!
//! Where the manifest and the code disagreed, the disagreements were resolved by
//! reading the decisions document and the manifest was corrected; each such
//! correction is annotated in `MANIFEST.md` and called out in the test that
//! pins the real behaviour.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use phronesis_mcp::graph::extract::{DEFAULT_WATCHLIST, extract_rust_at_module_with_ownership};
use phronesis_mcp::graph::model::Edge;
use phronesis_mcp::graph::ownership as own;
use phronesis_mcp::graph::ownership::config::OwnershipConfig;
use phronesis_mcp::graph::ownership::extract::{
    AST_EMITTABLE, CAPABILITY_AST_EXTRACTION, OPERAND_TEXT_CAP, REASON_COMPLETE, STATUS_AVAILABLE,
};
use phronesis_mcp::graph::unit::UnitContext;

// ── the arity table (§6.1, §6.2) ────────────────────────────────────────
//
// Transcribed from the spec's two relation tables. D18: "There is no arity
// validation anywhere — a wrong-arity edge produces no error, just a pattern
// that never matches. §13.1's arity tests must be written from scratch and are
// the only thing standing between a typo and silent nonsense."
const SPEC_ARITY: &[(&str, usize)] = &[
    (own::OWNERSHIP_SITE, 1),
    (own::OWNERSHIP_SITE_IN_FUNCTION, 2),
    (own::OWNERSHIP_SITE_SPAN, 4),
    (own::CLONE_SITE, 3),
    (own::FILTER_SITE, 2),
    (own::AWAIT_SITE, 1),
    (own::MUTATION_SITE, 3),
    (own::SYNC_LOCK_SITE, 3),
    (own::OWNERSHIP_EVIDENCE, 3),
    (own::OWNERSHIP_ANALYSIS_STATUS, 4),
    (own::FILTER_BEFORE_CLONE, 3),
    (own::CLONE_BEFORE_AWAIT, 3),
    (own::READ_BEFORE_MUTATION, 3),
    (own::LOCK_SCOPE_ENDS_BEFORE_AWAIT, 3),
];

/// `[function, <first site>, <second site>]` — the site *kinds* the spec fixes
/// for each ordering relation, in the order it fixes them. Reversing the two
/// site arguments of `read_before_mutation` would invert its meaning while
/// still type-checking and still matching arity.
const ORDERING_SHAPE: &[(&str, &str, &str)] = &[
    (own::FILTER_BEFORE_CLONE, "filter", "clone"),
    (own::CLONE_BEFORE_AWAIT, "clone", "await"),
    (own::READ_BEFORE_MUTATION, "clone", "mutation"),
    (own::LOCK_SCOPE_ENDS_BEFORE_AWAIT, "lock", "await"),
];

/// Site-kind label → the one kind-specific relation that site must carry.
const KIND_RELATION: &[(&str, &str)] = &[
    ("clone", own::CLONE_SITE),
    ("filter", own::FILTER_SITE),
    ("await", own::AWAIT_SITE),
    ("mutation", own::MUTATION_SITE),
    ("lock", own::SYNC_LOCK_SITE),
];

/// Relations the AST provider must never produce, whatever the input (D19).
const COMPILER_ONLY: &[&str] = &[
    "lock_scope_may_cross_await",
    "ownership_transfer",
    "borrow_live_across",
    "ownership_conflict_diagnostic",
    own::RESOLVED_TYPE,
];

// ── harness ─────────────────────────────────────────────────────────────

fn corpus_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ownership")
}

fn enabled() -> OwnershipConfig {
    OwnershipConfig {
        enabled: true,
        ..OwnershipConfig::disabled()
    }
}

/// One fixture, its source, and everything the extractor produced for it.
struct Fixture {
    name: String,
    path: String,
    source: String,
    edges: Vec<Edge>,
}

impl Fixture {
    fn load(name: &str, config: &OwnershipConfig) -> Self {
        let source = std::fs::read_to_string(corpus_dir().join(name))
            .unwrap_or_else(|error| panic!("fixture {name} must be readable: {error}"));
        let path = format!("src/{name}");
        let edges = extract_rust_at_module_with_ownership(
            &path,
            &source,
            DEFAULT_WATCHLIST,
            &UnitContext::default(),
            None,
            config,
        )
        .edges;
        Self {
            name: name.to_string(),
            path,
            source,
            edges,
        }
    }

    /// Every argument list emitted for one relation, in emission order.
    fn args(&self, relation: &str) -> Vec<&[String]> {
        self.edges
            .iter()
            .filter(|edge| edge.p == relation)
            .map(|edge| edge.a.as_slice())
            .collect()
    }

    fn ownership_edges(&self) -> Vec<&Edge> {
        self.edges
            .iter()
            .filter(|edge| own::OWNERSHIP_RELATIONS.contains(&edge.p.as_str()))
            .collect()
    }

    /// Site ids of one kind label, e.g. every `clone` site in the file.
    fn sites_of_kind(&self, kind: &str) -> Vec<&str> {
        self.args(own::OWNERSHIP_SITE)
            .into_iter()
            .filter_map(|args| args.first())
            .map(String::as_str)
            .filter(|site| site_kind(site) == kind)
            .collect()
    }

    /// `clone_site` / `mutation_site` / `sync_lock_site` operation by site id.
    fn operations(&self, relation: &str) -> BTreeMap<&str, &str> {
        self.args(relation)
            .into_iter()
            .filter_map(|args| Some((args.first()?.as_str(), args.get(1)?.as_str())))
            .collect()
    }

    /// Source text of a site's recorded span — what a human would be shown.
    fn span_text(&self, site: &str) -> &str {
        let args = self
            .args(own::OWNERSHIP_SITE_SPAN)
            .into_iter()
            .find(|args| args.first().map(String::as_str) == Some(site))
            .unwrap_or_else(|| panic!("{}: no span for site {site}", self.name));
        let start = byte_arg(&args[2]);
        let end = byte_arg(&args[3]);
        &self.source[start..end]
    }

    /// Short names of the functions the walk defined, for readable assertions.
    fn defined_functions(&self) -> BTreeSet<&str> {
        self.args("defines_fn")
            .into_iter()
            .filter_map(|args| args.get(1))
            .map(|id| short_name(id))
            .collect()
    }
}

fn fixture_names() -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(corpus_dir())
        .expect("the ownership fixture corpus directory must exist")
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|e| e.to_str()) == Some("rs"))
        .filter_map(|path| path.file_name().and_then(|n| n.to_str()).map(String::from))
        .collect();
    names.sort();
    names
}

fn corpus() -> Vec<Fixture> {
    let config = enabled();
    fixture_names()
        .iter()
        .map(|name| Fixture::load(name, &config))
        .collect()
}

fn load(name: &str) -> Fixture {
    Fixture::load(name, &enabled())
}

/// The function-id half of a site id (`<function-id>#ownership:<kind>:<byte>`).
fn site_function(site: &str) -> &str {
    site.split('#').next().unwrap_or_default()
}

/// The `<kind>` segment of a site id.
fn site_kind(site: &str) -> &str {
    let mut parts = site.rsplit(':');
    parts.next();
    parts.next().unwrap_or_default()
}

/// The anchor byte offset a site id embeds (D1).
fn site_anchor(site: &str) -> usize {
    site.rsplit(':')
        .next()
        .and_then(|byte| byte.parse().ok())
        .unwrap_or_else(|| panic!("site id must end in a decimal byte offset: {site}"))
}

/// Last `::` segment of a function id — the readable function name.
fn short_name(id: &str) -> &str {
    id.rsplit("::").next().unwrap_or(id)
}

fn byte_arg(raw: &str) -> usize {
    raw.parse()
        .unwrap_or_else(|_| panic!("byte offsets are decimal strings (D18), got {raw:?}"))
}

// ── the corpus itself ───────────────────────────────────────────────────

// A corpus test that silently found no corpus reports green forever, and every
// other assertion in this file is conditional on the sweep having something to
// sweep. This is the guard that makes the rest meaningful.
#[test]
fn the_fixture_corpus_is_present_and_every_file_parses() {
    let corpus = corpus();
    assert!(
        corpus.len() >= 23,
        "the documented corpus is 23 minimized fixtures; found {}: {:?}",
        corpus.len(),
        corpus.iter().map(|f| &f.name).collect::<Vec<_>>()
    );
    for fixture in &corpus {
        // An unparseable file yields `Extracted::unparseable()`, which carries
        // no edges at all — not even the `file_type` edge every parsed file
        // gets. That is exactly how a fixture with a syntax error would hide,
        // producing "no forbidden relations" for the wrong reason.
        assert!(
            fixture.edges.iter().any(|edge| edge.p == "file_type"),
            "{} did not parse; a fixture that fails to parse asserts nothing",
            fixture.name
        );
        // §3/Goal 3: "we looked and found nothing" must be distinguishable
        // from "we never looked", so the status is emitted even for a file
        // with no sites.
        let status = fixture.args(own::OWNERSHIP_ANALYSIS_STATUS);
        assert_eq!(
            status.len(),
            1,
            "{} must report exactly one ast_extraction status",
            fixture.name
        );
        assert_eq!(
            status[0].to_vec(),
            vec![
                fixture.path.clone(),
                CAPABILITY_AST_EXTRACTION.to_string(),
                STATUS_AVAILABLE.to_string(),
                REASON_COMPLETE.to_string(),
            ],
            "{} is a small fixture, so its analysis is complete and says so",
            fixture.name
        );
    }
}

// MANIFEST.md is the corpus's specification. A fixture with no entry is
// unspecified, and an entry with no fixture is an expectation nothing checks —
// both are how the corpus silently rots.
#[test]
fn every_fixture_has_a_manifest_entry_and_every_entry_has_a_fixture() {
    let manifest = std::fs::read_to_string(corpus_dir().join("MANIFEST.md"))
        .expect("the corpus MANIFEST must exist");
    let documented: BTreeSet<&str> = manifest
        .lines()
        .filter_map(|line| line.strip_prefix("## "))
        .map(str::trim)
        .filter(|heading| heading.ends_with(".rs"))
        .collect();
    let present: BTreeSet<String> = fixture_names().into_iter().collect();
    let present: BTreeSet<&str> = present.iter().map(String::as_str).collect();
    assert_eq!(
        documented, present,
        "MANIFEST.md headings and the fixture files must be the same set"
    );
}

// ── structural sweep ────────────────────────────────────────────────────

// D18: nothing in the graph or the engine validates arity. A relation emitted
// with the wrong number of arguments raises no error anywhere — it simply never
// matches a pattern again, which looks exactly like "this case does not occur".
// These numbers are transcribed from the spec's §6.1 and §6.2 tables.
#[test]
fn every_relation_the_corpus_emits_has_the_arity_the_spec_specifies() {
    let table: BTreeMap<&str, usize> = SPEC_ARITY.iter().copied().collect();
    assert_eq!(
        table.keys().copied().collect::<BTreeSet<_>>(),
        AST_EMITTABLE.iter().copied().collect::<BTreeSet<_>>(),
        "the arity table must cover exactly the relations the AST provider may \
         emit; a new relation is a graph-format change and needs a row here"
    );

    let mut exercised: BTreeSet<String> = BTreeSet::new();
    for fixture in corpus() {
        for edge in fixture.ownership_edges() {
            let expected = table.get(edge.p.as_str()).unwrap_or_else(|| {
                panic!(
                    "{}: {} has no arity in the spec table",
                    fixture.name, edge.p
                )
            });
            assert_eq!(
                edge.a.len(),
                *expected,
                "{}: {} must carry {expected} arguments, got {:?}",
                fixture.name,
                edge.p,
                edge.a
            );
            // `Edge::fact_id` joins arguments with U+001F, so an argument
            // containing that byte silently corrupts identity (D18).
            for argument in &edge.a {
                assert!(
                    !argument.contains('\u{1f}'),
                    "{}: {} argument {argument:?} contains the U+001F join byte",
                    fixture.name,
                    edge.p
                );
            }
            exercised.insert(edge.p.clone());
        }
    }
    assert!(
        exercised.len() >= 12,
        "the corpus must actually exercise the relation set rather than pass \
         by emitting almost nothing: {exercised:?}"
    );
}

// D12: `src` is the only compaction key. `store::compact` filters fresh edges
// with `d: true` and discards them without erroring, and an ownership edge with
// an empty `src` is unreachable by file replacement, so it becomes permanently
// stale the first time its file changes.
#[test]
fn every_ownership_edge_is_a_base_edge_naming_its_own_fixture_as_provenance() {
    for fixture in corpus() {
        for edge in fixture.ownership_edges() {
            assert!(
                !edge.d,
                "{}: {} was emitted as derived; `store::compact` would discard \
                 it with no error (D12)",
                fixture.name, edge.p
            );
            assert_eq!(
                edge.src, fixture.path,
                "{}: every ownership edge carries its own source file, or \
                 provenance degrades to graph:structural (D12)",
                fixture.name
            );
        }
    }
}

// §5.2/§6.1: the site id is argument 0 of every per-site relation, and the
// relation that carries a site's *kind* must agree with the kind the id
// embeds. A clone site declared with a `filter_site` edge would render as the
// wrong operation for the rest of the graph's life.
#[test]
fn every_site_carries_exactly_one_kind_one_span_one_function_and_one_evidence() {
    let kinds: BTreeMap<&str, &str> = KIND_RELATION.iter().copied().collect();
    for fixture in corpus() {
        let declared: Vec<&str> = fixture
            .args(own::OWNERSHIP_SITE)
            .into_iter()
            .filter_map(|args| args.first())
            .map(String::as_str)
            .collect();
        assert_eq!(
            declared.iter().collect::<BTreeSet<_>>().len(),
            declared.len(),
            "{}: two sites shared one id; D1's anchor byte exists to prevent \
             exactly this in a chain",
            fixture.name
        );

        for site in &declared {
            let kind = site_kind(site);
            let expected = kinds
                .get(kind)
                .unwrap_or_else(|| panic!("{}: unknown site kind in {site}", fixture.name));
            let of_kind: Vec<&[String]> = fixture
                .args(expected)
                .into_iter()
                .filter(|args| args.first().map(String::as_str) == Some(*site))
                .collect();
            assert_eq!(
                of_kind.len(),
                1,
                "{}: {site} must carry exactly one {expected}",
                fixture.name
            );
            for (label, relation) in KIND_RELATION {
                if label == &kind {
                    continue;
                }
                assert!(
                    !fixture
                        .args(relation)
                        .iter()
                        .any(|args| args.first().map(String::as_str) == Some(*site)),
                    "{}: {site} is a {kind} site but also carries {relation}",
                    fixture.name
                );
            }
            for one_of in [own::OWNERSHIP_SITE_IN_FUNCTION, own::OWNERSHIP_SITE_SPAN] {
                assert_eq!(
                    fixture
                        .args(one_of)
                        .iter()
                        .filter(|args| args.first().map(String::as_str) == Some(*site))
                        .count(),
                    1,
                    "{}: {site} must have exactly one {one_of}",
                    fixture.name
                );
            }
            let evidence: Vec<&[String]> = fixture
                .args(own::OWNERSHIP_EVIDENCE)
                .into_iter()
                .filter(|args| args.first().map(String::as_str) == Some(*site))
                .collect();
            assert_eq!(
                evidence.len(),
                1,
                "{}: {site} must state at what strength it was observed",
                fixture.name
            );
            assert_eq!(
                (evidence[0][1].as_str(), evidence[0][2].as_str()),
                ("ast", own::PROVIDER_TREE_SITTER_RUST),
                "{}: {site} was observed by tree-sitter, at ast strength — \
                 never anything stronger",
                fixture.name
            );
        }

        // Nothing may reference a site that was never declared.
        let declared: BTreeSet<&str> = declared.into_iter().collect();
        for (_, relation) in KIND_RELATION {
            for args in fixture.args(relation) {
                assert!(
                    declared.contains(args[0].as_str()),
                    "{}: {relation} names undeclared site {}",
                    fixture.name,
                    args[0]
                );
            }
        }
    }
}

// §6.1: `ownership_site_span(site, file, start_byte, end_byte)`. Byte offsets
// are decimal strings (D18) and UTF-8 byte offsets (§7.4). A span that does not
// address its own file, or lands mid-character, renders as garbage in the one
// surface a human uses to check the claim.
#[test]
fn every_span_addresses_real_bytes_of_the_file_it_names() {
    for fixture in corpus() {
        for args in fixture.args(own::OWNERSHIP_SITE_SPAN) {
            let (site, file) = (args[0].as_str(), args[1].as_str());
            assert_eq!(
                file, fixture.path,
                "{}: a span must name the file it was found in",
                fixture.name
            );
            let (start, end) = (byte_arg(&args[2]), byte_arg(&args[3]));
            assert!(
                start < end && end <= fixture.source.len(),
                "{}: {site} span {start}..{end} is outside the file",
                fixture.name
            );
            assert!(
                fixture.source.is_char_boundary(start) && fixture.source.is_char_boundary(end),
                "{}: {site} span {start}..{end} splits a UTF-8 character (§7.4)",
                fixture.name
            );
            // D1: the id anchors on the operation-name token, while the span
            // records the whole anchoring expression. The anchor must lie
            // inside the span, or the two are describing different code.
            let anchor = site_anchor(site);
            assert!(
                start <= anchor && anchor < end,
                "{}: {site} anchors at {anchor}, outside its own span {start}..{end}",
                fixture.name
            );
        }
    }
}

// D7: operand text is whitespace-collapsed and *never truncated* — over the cap
// it becomes an opaque digest, because a truncated expression reads as a real
// but wrong expression. §6.1 also fixes which argument slot holds the operand.
#[test]
fn every_operand_is_collapsed_source_text_or_an_opaque_digest() {
    for fixture in corpus() {
        let operands = fixture
            .args(own::CLONE_SITE)
            .into_iter()
            .chain(fixture.args(own::MUTATION_SITE))
            .filter_map(|args| args.get(2).cloned())
            .chain(
                fixture
                    .args(own::FILTER_SITE)
                    .into_iter()
                    .filter_map(|args| args.get(1).cloned()),
            );
        for operand in operands {
            assert!(
                !operand.is_empty(),
                "{}: an operand slot was left empty",
                fixture.name
            );
            if let Some(digest) = operand.strip_prefix("sha256:") {
                assert_eq!(
                    digest.len(),
                    16,
                    "{}: D7 fixes the marker at sha256: plus 16 hex characters, got {operand:?}",
                    fixture.name
                );
                assert!(
                    digest
                        .chars()
                        .all(|c| c.is_ascii_hexdigit() && !c.is_uppercase()),
                    "{}: the digest is lowercase hex, got {operand:?}",
                    fixture.name
                );
                continue;
            }
            assert!(
                operand.len() <= OPERAND_TEXT_CAP,
                "{}: {} bytes of operand text was stored verbatim; over the cap \
                 D7 requires a digest, never a truncation",
                fixture.name,
                operand.len()
            );
            assert!(
                !operand.contains('\n') && !operand.contains("  "),
                "{}: operand {operand:?} is not whitespace-collapsed (D7)",
                fixture.name
            );
        }
    }
}

// §6.2 fixes both the arity *and the order* of the ordering relations. Swapping
// the two site arguments of `read_before_mutation` inverts its meaning while
// still matching arity, and nothing else in the codebase would notice.
#[test]
fn every_ordering_relation_names_a_function_then_its_two_sites_in_spec_order() {
    let mut exercised: BTreeSet<&str> = BTreeSet::new();
    for fixture in corpus() {
        for (relation, first_kind, second_kind) in ORDERING_SHAPE {
            for args in fixture.args(relation) {
                exercised.insert(*relation);
                let (function, first, second) =
                    (args[0].as_str(), args[1].as_str(), args[2].as_str());
                assert!(
                    !function.contains('#'),
                    "{}: {relation} argument 0 is a function id, got the site {function}",
                    fixture.name
                );
                assert_eq!(
                    (site_function(first), site_function(second)),
                    (function, function),
                    "{}: {relation} may only relate two sites of the function it names",
                    fixture.name
                );
                assert_eq!(
                    (site_kind(first), site_kind(second)),
                    (*first_kind, *second_kind),
                    "{}: {relation} is [function, {first_kind} site, {second_kind} site] (§6.2)",
                    fixture.name
                );
                assert!(
                    site_anchor(first) < site_anchor(second),
                    "{}: {relation} names {first} after {second}; every one of \
                     these relations records an observed order",
                    fixture.name
                );
            }
        }
    }
    assert_eq!(
        exercised.len(),
        ORDERING_SHAPE.len(),
        "each ordering relation must be exercised by the corpus, or its \
         argument order is pinned by nothing: {exercised:?}"
    );
}

// ── D19: the AST provider cannot make a compiler claim ──────────────────

// D19 in its literal form, and the test that actually satisfies §13.1's "AST
// extraction never emits MIR-only relations". The derivation code sits directly
// beside the AST code, so a copy-paste is all it takes to promote AST evidence
// into a compiler claim — the single worst failure this feature can have.
#[test]
fn the_whole_corpus_emits_no_relation_outside_ast_emittable() {
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for fixture in corpus() {
        for edge in &fixture.edges {
            if COMPILER_ONLY.contains(&edge.p.as_str()) {
                panic!(
                    "{} produced the compiler-only relation {} from syntax alone",
                    fixture.name, edge.p
                );
            }
            if own::OWNERSHIP_RELATIONS.contains(&edge.p.as_str()) {
                assert!(
                    AST_EMITTABLE.contains(&edge.p.as_str()),
                    "{}: the AST provider may not emit {} (D19)",
                    fixture.name,
                    edge.p
                );
                seen.insert(edge.p.clone());
            }
        }
    }
    assert!(
        seen.len() >= 12,
        "the corpus must exercise the relation set, not pass by emitting \
         nothing: {seen:?}"
    );
    for forbidden in COMPILER_ONLY {
        assert!(
            !seen.contains(*forbidden),
            "{forbidden} is a compiler claim and must be unreachable from AST evidence"
        );
    }
}

// The specific case D19 exists for: a guard that genuinely *is* live across an
// await. A lexical analysis can see the containment, and the tempting edge is
// `lock_scope_may_cross_await` — which §6.2 reserves for MIR or an explicit
// rustc diagnostic. The correct output is the two sites and no relation at all:
// silence, not a claim of safety and not a claim of danger.
#[test]
fn a_guard_genuinely_live_across_an_await_produces_no_crossing_claim() {
    let fixture = load("guard_live_across_await.rs");
    assert_eq!(
        fixture.sites_of_kind("lock").len(),
        1,
        "the acquisition itself is still observed"
    );
    assert_eq!(
        fixture.sites_of_kind("await").len(),
        1,
        "the await itself is still observed"
    );
    assert!(
        fixture.args(own::LOCK_SCOPE_ENDS_BEFORE_AWAIT).is_empty(),
        "the guard's block encloses the await, so its scope does not end first"
    );
    assert!(
        !fixture
            .edges
            .iter()
            .any(|edge| edge.p == "lock_scope_may_cross_await"),
        "AST containment alone must never produce the crossing relation (§6.2)"
    );
}

// ── D21: function-id set equality ───────────────────────────────────────

// D21's mandatory guard. A site id embeds a function id, and a site whose
// function id diverges from `defines_fn` is unresolvable — violating §15 — with
// every divergence source silent: a generic impl keeps its literal `Foo<Bar>`
// segment, a trait default method carries no trait segment, `#[path]` overrides
// the module, nested modules stack. Set equality is the only cheap way to catch
// D13 being violated by an id reconstructed instead of taken from the walk.
#[test]
fn the_function_id_fixture_pins_set_equality_between_sites_and_defines_fn() {
    let fixture = load("function_id_divergence.rs");
    let defined: BTreeSet<&str> = fixture
        .args("defines_fn")
        .into_iter()
        .filter_map(|args| args.get(1))
        .map(String::as_str)
        .collect();
    let owned: BTreeSet<&str> = fixture
        .args(own::OWNERSHIP_SITE_IN_FUNCTION)
        .into_iter()
        .filter_map(|args| args.get(1))
        .map(String::as_str)
        .collect();
    assert_eq!(
        owned, defined,
        "every function in the divergence fixture contains a clone, so the two \
         id sets must be equal — not merely overlapping"
    );
    assert_eq!(
        defined.len(),
        6,
        "the fixture exercises six contexts: generic impl, plain impl, trait \
         impl, default-bodied trait method, nested module, free function: {defined:?}"
    );
    // Named individually, because equality between two empty sets, or between
    // two sets that both lost the same member, would still pass above.
    let generic = defined
        .iter()
        .find(|id| id.contains("generic_impl_method"))
        .copied()
        .unwrap_or_default();
    assert!(
        generic.contains("GenericFoo<i32>"),
        "a generic impl keeps its literal segment; an id reconstructed from the \
         file path would normalise it away: {generic}"
    );
    for expected in [
        "Foo::plain_impl_method",
        "Foo::trait_impl_method",
        "a::b::deeply_nested",
    ] {
        assert!(
            defined.iter().any(|id| id.ends_with(expected)),
            "{expected} must be one of the defined ids: {defined:?}"
        );
    }
}

// The same guard over the whole corpus, so that no fixture can introduce an
// unresolvable site without a failure. §15: every ownership site resolves to a
// real graph function.
#[test]
fn every_site_in_the_corpus_resolves_to_a_function_the_same_walk_defined() {
    for fixture in corpus() {
        let defined: BTreeSet<&str> = fixture
            .args("defines_fn")
            .into_iter()
            .filter_map(|args| args.get(1))
            .map(String::as_str)
            .collect();
        for args in fixture.args(own::OWNERSHIP_SITE_IN_FUNCTION) {
            let (site, function) = (args[0].as_str(), args[1].as_str());
            assert!(
                defined.contains(function),
                "{}: site function {function} is not a defines_fn identity; \
                 defined: {defined:?}",
                fixture.name
            );
            assert_eq!(
                site_function(site),
                function,
                "{}: the id a site embeds must be the function it names",
                fixture.name
            );
        }
    }
}

// §13.2's opt-in requirement, at the extractor. The same corpus run with the
// disabled configuration must produce nothing at all — the guard against an
// ownership code path that never consults the config.
#[test]
fn the_whole_corpus_emits_no_ownership_edge_when_the_configuration_is_disabled() {
    let disabled = OwnershipConfig::disabled();
    for name in fixture_names() {
        let fixture = Fixture::load(&name, &disabled);
        let leaked = fixture.ownership_edges();
        assert!(
            leaked.is_empty(),
            "{name} emitted {} ownership edges with the enrichment disabled",
            leaked.len()
        );
        assert!(
            fixture.edges.iter().any(|edge| edge.p == "file_type"),
            "{name} must still be walked for ordinary graph facts"
        );
    }
}

// ── per-case expectations from the MANIFEST ─────────────────────────────

// §7.5's negative control, and D11's contrast: `syntax/rust/hazards.rs` finds
// these by substring (`value_text.contains(".lock()")`) and would fail this
// fixture outright. The ownership extractor only ever classifies nodes it
// walked, so a `.clone()` that exists only in a comment or a string literal is
// not an operation — it is not even a node.
#[test]
fn clone_lock_and_await_inside_comments_and_strings_produce_no_site_at_all() {
    let fixture = load("comments_and_strings.rs");
    assert!(
        fixture.args(own::OWNERSHIP_SITE).is_empty(),
        "text inside comments and string literals is not an observed operation: {:?}",
        fixture.args(own::OWNERSHIP_SITE)
    );
    // The fixture has to actually contain the bait, or it proves nothing.
    assert!(
        fixture.source.matches(".clone()").count() >= 5
            && fixture.source.contains(".lock()")
            && fixture.source.contains("r#\""),
        "the negative control must contain .clone()/.lock()/.await in a line \
         comment, a block comment, a doc comment, a string, and a raw string"
    );
    assert!(
        !fixture.defined_functions().is_empty(),
        "the file still defines a function, so 'no sites' is not 'no walk'"
    );
}

// §6.2 and D2 together: `filter_before_clone` requires a shared *receiver
// chain*, and D2 makes intervening adapters explicitly non-breaking. This is
// the difference between the real-world performance incident and two statements that
// happen to be adjacent.
#[test]
fn filter_before_clone_holds_through_an_intervening_adapter_and_without_one() {
    let fixture = load("filter_before_clone.rs");
    let operations = fixture.operations(own::CLONE_SITE);
    let related: Vec<(&str, &str)> = fixture
        .args(own::FILTER_BEFORE_CLONE)
        .into_iter()
        .map(|args| {
            (
                short_name(site_function(&args[1])),
                *operations
                    .get(args[2].as_str())
                    .unwrap_or_else(|| panic!("clone end {} has no clone_site", args[2])),
            )
        })
        .collect();

    // `chained` and `chained_with_intervening` both put `.map(..)` between the
    // filter and the clone; `chained_direct` has no adapter at all. All three
    // must relate the filter to the `.cloned()` call (D2).
    for function in ["chained", "chained_with_intervening", "chained_direct"] {
        assert!(
            related.contains(&(function, "cloned")),
            "{function}: a filter in the clone's receiver chain must produce the \
             relation, adapter or not: {related:?}"
        );
        // D5: the trailing `.collect()` is a clone site too, and the same
        // filter is in *its* receiver chain, so it relates as well. The
        // MANIFEST originally forbade this; it was corrected, because D5 makes
        // every `collect` a clone site unconditionally.
        assert!(
            related.contains(&(function, "collect")),
            "{function}: the trailing collect is a clone site whose receiver \
             chain also contains the filter (D5): {related:?}"
        );
    }
    assert_eq!(
        related.len(),
        6,
        "three functions, each relating its filter to two clone-family calls: {related:?}"
    );
}

// D2's negative half, and §6.2's "mere line ordering is insufficient". Both
// adversarial fixtures pair a filter with a clone that shares no chain with it.
//
// The negative needs care, because both files spell their *filter* statement
// `xs.iter().filter(p).collect()`, and D5 makes every `collect` a clone site —
// so that statement genuinely is a filter in a clone's receiver chain and
// correctly relates. What is adversarial is the `.cloned()` / `.clone()` in the
// *other* statement. The assertion is therefore exact: no clone site whose
// operation is not `collect` may ever be named as the clone end.
#[test]
fn a_filter_and_a_clone_in_separate_statements_are_never_related() {
    for name in ["unrelated_filter_and_clone.rs", "clone_before_filter.rs"] {
        let fixture = load(name);
        let operations = fixture.operations(own::CLONE_SITE);
        // Both sites must still be observed, or the negative is
        // indistinguishable from the extractor not looking.
        assert!(
            !fixture.args(own::FILTER_SITE).is_empty(),
            "{name}: the filter site itself must still be recorded"
        );
        let unchained: BTreeSet<&str> = operations
            .iter()
            .filter(|(_, operation)| **operation != "collect")
            .map(|(site, _)| *site)
            .collect();
        assert!(
            !unchained.is_empty(),
            "{name}: the fixture must contain an explicit clone outside the \
             filter's chain, or it is not adversarial"
        );
        for args in fixture.args(own::FILTER_BEFORE_CLONE) {
            assert!(
                !unchained.contains(args[2].as_str()),
                "{name}: {} shares no receiver chain with the filter, so line \
                 order alone must not relate them (D2)",
                args[2]
            );
        }
    }
}

// D20: an awaited acquisition is an async lock. The distinction is structural —
// whether the acquisition is the direct operand of an `await_expression` —
// because §7.10 forbids inferring the receiver's type from how it is spelled,
// and §7.5 forbids the `source.contains("std::sync")` test `hazards.rs` uses.
#[test]
fn an_awaited_lock_acquisition_is_never_a_sync_lock_site() {
    let fixture = load("async_lock_not_sync.rs");
    let acquisitions: BTreeMap<&str, &str> = fixture
        .args(own::SYNC_LOCK_SITE)
        .into_iter()
        .map(|args| (short_name(site_function(&args[0])), args[1].as_str()))
        .collect();
    assert_eq!(
        acquisitions,
        [
            ("sync_lock", "lock"),
            ("sync_read", "read"),
            ("sync_write", "write")
        ]
        .into_iter()
        .collect::<BTreeMap<_, _>>(),
        "only the three non-awaited acquisitions are synchronous locks"
    );
    // The async halves must still have been walked — an await site each — or
    // "no sync_lock_site" would just mean the functions were skipped.
    let awaited: BTreeSet<&str> = fixture
        .sites_of_kind("await")
        .into_iter()
        .map(|site| short_name(site_function(site)))
        .collect();
    assert_eq!(
        awaited,
        ["async_lock", "async_read", "async_write"]
            .into_iter()
            .collect::<BTreeSet<_>>(),
        "each async variant was walked and its await observed"
    );
}

// D22: a site inside a `macro_rules!` body has no enclosing function and
// therefore no canonical function id, so §7.11 requires emitting nothing. The
// ordinary `.clone()` in the same file must still be observed, or the rule is
// indistinguishable from the extractor skipping the file.
#[test]
fn a_macro_definition_body_emits_nothing_while_the_real_function_still_does() {
    let fixture = load("macro_definition_body.rs");
    let clones = fixture.args(own::CLONE_SITE);
    assert_eq!(
        clones.len(),
        1,
        "only the clone in the real function body is a site: {clones:?}"
    );
    assert_eq!(
        (clones[0][1].as_str(), clones[0][2].as_str()),
        ("clone", "data"),
        "the observed clone is the one written in the function, not in the macro"
    );
    assert!(
        short_name(site_function(&clones[0][0])) == "macro_invocation_with_own_clone",
        "the site belongs to the ordinary function"
    );
    assert!(
        fixture.args(own::SYNC_LOCK_SITE).is_empty(),
        "the `.lock()` inside the macro definition body has no enclosing \
         function and must produce no site (D22)"
    );
    // The fixture must really contain the bait inside the macro.
    assert!(
        fixture.source.contains("macro_rules!") && fixture.source.matches(".clone()").count() >= 2,
        "the fixture must have a clone inside the macro definition and one outside"
    );
}

// D8's macro incompleteness, stated positively: expansion is never analysed, so
// a macro whose *expansion* contains the only clone in the file yields nothing.
// This is an absence of evidence, which the graph is allowed to have; it is not
// a claim that the function is clone-free.
#[test]
fn a_clone_that_exists_only_inside_a_macro_expansion_is_not_observed() {
    let fixture = load("macro_generated_calls.rs");
    assert!(
        fixture.args(own::OWNERSHIP_SITE).is_empty(),
        "the only `.clone()` is in the macro body and the invocation is not \
         expanded (D8/D22): {:?}",
        fixture.args(own::OWNERSHIP_SITE)
    );
    assert!(
        fixture
            .defined_functions()
            .contains("macro_generated_calls"),
        "the surrounding function was still walked"
    );
}

// D6 case 2. §6.2 defines the relation lexically, but §12 demands an
// adversarial `drop(guard)` fixture, which is semantic — as written the fixture
// could not pass, so D6 added the explicit-drop path. The guard's block here
// extends past the await; only the `drop(g)` between the binding and the await
// makes the relation true.
#[test]
fn an_explicit_drop_before_the_await_ends_the_lock_scope() {
    let fixture = load("explicit_drop_before_await.rs");
    let relations = fixture.args(own::LOCK_SCOPE_ENDS_BEFORE_AWAIT);
    assert_eq!(
        relations.len(),
        1,
        "the dropped guard's scope ends before the await: {relations:?}"
    );
    let lock = fixture.args(own::SYNC_LOCK_SITE);
    assert_eq!(
        lock[0][2], "g",
        "the relation is only reachable because the guard is bound to a name D6 \
         can match against the drop argument"
    );
    assert_eq!(relations[0][1], lock[0][0], "argument 1 is the lock site");
    assert!(
        fixture.source.contains("drop(g);"),
        "the fixture must actually drop the guard, or the lexical path alone \
         would explain the relation"
    );
}

// §6.2's lexical path, and the reason the relation is worth having: two guards
// in two nested blocks that both close before the await.
#[test]
fn a_guard_block_that_closes_before_the_await_ends_the_lock_scope() {
    let fixture = load("lock_scope_ends_before_await.rs");
    let locks = fixture.sites_of_kind("lock");
    assert_eq!(locks.len(), 2, "two acquisitions, two guards: {locks:?}");
    let relations = fixture.args(own::LOCK_SCOPE_ENDS_BEFORE_AWAIT);
    assert_eq!(
        relations.len(),
        2,
        "each guard's block ends before the single await: {relations:?}"
    );
    assert_eq!(
        relations
            .iter()
            .map(|args| args[1].as_str())
            .collect::<BTreeSet<_>>(),
        locks.iter().copied().collect::<BTreeSet<_>>(),
        "both guards are related, not just the first"
    );
}

// §7.9 and D6 case 3: an unbound temporary guard names no binding, but its
// drop point *is* knowable — Rust releases it at the end of the enclosing
// statement — so a scope conclusion follows when that statement precedes an
// await. The last function is the boundary control: a temporary `match`
// scrutinee is live across the whole match, so an await inside it yields
// nothing, which is what proves the statement boundary was used and not the
// scrutinee expression.
#[test]
fn an_unbound_temporary_guard_ends_at_its_statement_but_not_across_a_match() {
    let fixture = load("unbound_temporary_guard.rs");
    let locks = fixture.args(own::SYNC_LOCK_SITE);
    assert_eq!(locks.len(), 4, "every acquisition is observed: {locks:?}");
    assert!(
        locks.iter().all(|args| args[2].is_empty()),
        "an unbound guard records the empty string, not an invented name (§7.9): {locks:?}"
    );
    let scopes = fixture.args(own::LOCK_SCOPE_ENDS_BEFORE_AWAIT);
    assert_eq!(
        scopes.len(),
        1,
        "only the statement that closes before its await concludes: {scopes:?}"
    );
    assert!(
        scopes[0][0].ends_with("unbound_temporary_released_before_await"),
        "and it is the released-before-await function, not the match scrutinee \
         nor the post-await acquisition: {scopes:?}"
    );
}

// The grammar case a naive implementation gets wrong. `x.collect::<Vec<_>>()`
// parses as `call_expression { function: generic_function { function:
// field_expression } }`, so code that tests `function.kind() ==
// "field_expression"` directly — as `counts.rs` still does — misses every
// turbofish call, which is nearly every `collect` ever written.
#[test]
fn a_turbofish_collect_is_detected_despite_the_generic_function_wrapper() {
    let fixture = load("turbofish_collect.rs");
    let collects: Vec<&str> = fixture
        .operations(own::CLONE_SITE)
        .into_iter()
        .filter(|(_, operation)| *operation == "collect")
        .map(|(site, _)| site)
        .collect();
    assert_eq!(
        collects.len(),
        3,
        "all three collect calls are clone sites (D5): {collects:?}"
    );
    let turbofished: Vec<&str> = collects
        .iter()
        .copied()
        .filter(|site| fixture.span_text(site).contains("collect::<"))
        .collect();
    assert_eq!(
        turbofished.len(),
        2,
        "the two turbofished collects are exactly the ones a field_expression-only \
         implementation would miss: {turbofished:?}"
    );
    // The turbofish must not break the receiver chain either — the third
    // function ends its filter/clone chain in a turbofish collect.
    let filters = fixture.sites_of_kind("filter");
    assert_eq!(filters.len(), 2, "both filters observed: {filters:?}");
    assert_eq!(
        fixture.args(own::FILTER_BEFORE_CLONE).len(),
        3,
        "the receiver chain is walked through `generic_function`, so the \
         turbofished collect still relates to the filter above it"
    );
}

// D7's whole point. An operand over the 240-byte cap becomes a digest, never a
// truncation: a truncated expression reads as a real but wrong expression, and
// a reader has no way to tell it was cut.
#[test]
fn an_over_cap_operand_becomes_a_digest_and_never_a_truncated_expression() {
    let fixture = load("long_operand.rs");
    let operations = fixture.operations(own::CLONE_SITE);
    let digests: Vec<&str> = fixture
        .args(own::CLONE_SITE)
        .into_iter()
        .filter(|args| args[2].starts_with("sha256:"))
        .map(|args| args[0].as_str())
        .collect();
    assert_eq!(
        digests.len(),
        1,
        "exactly one operand in the fixture exceeds the cap after whitespace \
         collapse — the twelve-call chain in long_operand_dot_clone. (The \
         MANIFEST also claimed long_operand_clone's chain was over the cap; it \
         normalises to well under 240 bytes, and the entry was corrected.): \
         {digests:?}"
    );
    assert_eq!(
        short_name(site_function(digests[0])),
        "long_operand_dot_clone",
        "the digest belongs to the twelve-call chain"
    );
    let digest = fixture
        .args(own::CLONE_SITE)
        .into_iter()
        .find(|args| args[0] == digests[0])
        .map(|args| args[2].clone())
        .unwrap_or_default();
    assert!(
        !digest.contains("get_first_compound_component_name"),
        "the marker is opaque; a prefix of the real expression would read as a \
         real but wrong expression: {digest}"
    );
    // The thing being digested really is over the cap, so the branch was taken
    // for the documented reason rather than by accident.
    let collapsed = fixture
        .span_text(digests[0])
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        collapsed.len() > OPERAND_TEXT_CAP,
        "the fixture's long operand must actually exceed {OPERAND_TEXT_CAP} bytes, got {}",
        collapsed.len()
    );
    // And the short one is stored verbatim, so the cap is a cap and not a
    // blanket digest.
    assert!(
        operations.values().any(|operation| *operation == "clone"),
        "the short case is still an ordinary clone"
    );
    assert!(
        fixture
            .args(own::CLONE_SITE)
            .iter()
            .any(|args| args[2] == "small"),
        "an operand under the cap is stored as its own source text"
    );
}

// §7.2: a bodyless trait method is a `function_signature_item`, a distinct node
// kind with no body to observe and no `defines_fn` to hang a site on. The
// default-bodied method beside it is an ordinary function and must still be
// walked, or "nothing from the signature" would just be "nothing from the file".
#[test]
fn a_bodyless_trait_signature_emits_nothing_while_a_default_body_does() {
    let fixture = load("bodyless_trait_items.rs");
    let sites = fixture.args(own::OWNERSHIP_SITE);
    assert!(
        !sites.is_empty(),
        "the trait's default-bodied method contains clone-family calls"
    );
    for args in &sites {
        assert_eq!(
            short_name(site_function(&args[0])),
            "default_method",
            "only the default-bodied method has a body to observe: {}",
            args[0]
        );
    }
    // Two clone-family sites, not one: `.to_string()` is a clone operation per
    // §6.1's list as much as `.clone()` is. The MANIFEST said one; corrected.
    let operations: BTreeSet<&str> = fixture.operations(own::CLONE_SITE).into_values().collect();
    assert_eq!(
        operations,
        ["to_string", "clone"].into_iter().collect::<BTreeSet<_>>(),
        "both clone-family calls in the default body are observed"
    );
    assert!(
        fixture.source.contains("fn bodyless_method(&self) -> i32;"),
        "the fixture must actually contain a bodyless signature"
    );
}

// §7.6 and D5: the five clone-family operations are not interchangeable costs,
// so `operation` preserves exactly what was written. Collapsing them to
// "clone" would erase the only distinction the AST layer can honestly make.
#[test]
fn each_clone_family_operation_is_recorded_under_the_name_that_was_written() {
    let fixture = load("clone_operation_kinds.rs");
    let operations: BTreeSet<&str> = fixture.operations(own::CLONE_SITE).into_values().collect();
    assert_eq!(
        operations,
        ["clone", "cloned", "to_owned", "to_string", "collect"]
            .into_iter()
            .collect::<BTreeSet<_>>(),
        "all five distinct operations are preserved, none conflated"
    );
    // Seven sites over five functions, not five: two of the functions write
    // `.cloned().collect()`, which is two clone-family calls in one chain. The
    // MANIFEST claimed one per function; corrected.
    assert_eq!(
        fixture.args(own::CLONE_SITE).len(),
        7,
        "every clone-family call is its own site, including both halves of \
         `.cloned().collect()`"
    );
}

// D3: matching a *method name* against a closed list is an observation of what
// was written, never an inference about the receiver's type (§7.10). The
// closed list plus assignment through a projection is the whole of it — and a
// plain assignment to a bare local is deliberately not a mutation.
#[test]
fn mutation_sites_come_from_the_closed_list_and_from_projections_only() {
    let fixture = load("mutation_kinds.rs");
    let observed: BTreeMap<&str, &str> = fixture
        .args(own::MUTATION_SITE)
        .into_iter()
        .map(|args| (short_name(site_function(&args[0])), args[1].as_str()))
        .collect();
    assert_eq!(
        observed,
        [
            ("mutation_get_mut", "get_mut"),
            ("mutation_iter_mut", "iter_mut"),
            ("mutation_field_assignment", "="),
            ("mutation_index_assignment", "="),
        ]
        .into_iter()
        .collect::<BTreeMap<_, _>>(),
        "the closed method list and assignment through a field or index \
         projection, and nothing else"
    );
    // Two negative controls, both bare-local assignments. The MANIFEST expected
    // `mutation_compound_assignment` to be the fifth positive, describing it as
    // `self.n += 1`; the fixture actually writes `n += 1` against a bare local,
    // which D3 excludes exactly like the plain assignment beside it. The
    // MANIFEST was corrected rather than the assertion weakened. (Compound
    // assignment *through* a projection is pinned by the extractor's own unit
    // test `assignment_is_a_mutation_site_only_through_a_projection`.)
    for bare_local in [
        "mutation_compound_assignment",
        "mutation_plain_assignment_is_not_mutation",
    ] {
        assert!(
            fixture.defined_functions().contains(bare_local),
            "{bare_local} must exist, or the negative proves nothing"
        );
        assert!(
            !observed.contains_key(bare_local),
            "{bare_local} assigns to a bare local, which D3 excludes"
        );
    }
    let places: BTreeSet<&str> = fixture
        .args(own::MUTATION_SITE)
        .into_iter()
        .map(|args| args[2].as_str())
        .collect();
    assert!(
        places.contains("h.x") && places.contains("v[0]"),
        "the place records the projection that was assigned through: {places:?}"
    );
}

// D4 and §6.2: the read and the mutation must share a *syntactically identified
// root place*. `self.party.members.clone()` and
// `self.party.members.get_mut(0)` both root at `self`, which is the coarse
// grouping D4 deliberately accepts and D8 names as a false-positive class.
#[test]
fn read_before_mutation_relates_two_sites_through_their_shared_root_place() {
    let fixture = load("read_before_mutation.rs");
    let relations = fixture.args(own::READ_BEFORE_MUTATION);
    assert_eq!(
        relations.len(),
        1,
        "one snapshot, one mutation, one relation: {relations:?}"
    );
    let read = fixture
        .args(own::CLONE_SITE)
        .into_iter()
        .find(|args| args[0] == relations[0][1])
        .map(|args| args[2].clone())
        .unwrap_or_default();
    let mutated = fixture
        .args(own::MUTATION_SITE)
        .into_iter()
        .find(|args| args[0] == relations[0][2])
        .map(|args| args[2].clone())
        .unwrap_or_default();
    assert_eq!(
        (read.as_str(), mutated.as_str()),
        ("self.party.members", "self.party.members"),
        "both sides root at `self`, which is what D4 matches on"
    );
}

// §6.2 and D8: `clone_before_await` is *ordering* evidence and nothing more. It
// does not claim the cloned value crosses the suspension, and an early return,
// an unmatched arm, or an unentered loop between the two sites is invisible to
// it. Five control-flow shapes, and the relation holds in every one.
#[test]
fn clone_before_await_is_lexical_ordering_across_every_control_flow_shape() {
    let fixture = load("control_flow_boundaries.rs");
    let related: BTreeSet<&str> = fixture
        .args(own::CLONE_BEFORE_AWAIT)
        .into_iter()
        .map(|args| short_name(&args[0]))
        .collect();
    assert_eq!(
        related,
        [
            "control_flow_early_return",
            "control_flow_match",
            "control_flow_loop",
            "control_flow_closure",
            "control_flow_nested_async",
        ]
        .into_iter()
        .collect::<BTreeSet<_>>(),
        "the relation is lexical, so no control-flow boundary suppresses it"
    );
    // Six relations, not five: `control_flow_nested_async` contains two awaits
    // — the one inside the async block and the one applied to the block — and
    // the clone precedes both. The MANIFEST said one per function; corrected.
    assert_eq!(
        fixture.args(own::CLONE_BEFORE_AWAIT).len(),
        6,
        "every (earlier clone, later await) pair in a function is one relation"
    );
}

// §7.6 stated as a negative: two clones with identical syntax, one of an `i32`
// and one of a 10,000-byte `Vec`. Syntax cannot tell them apart, so the graph
// must record both without ranking them — no size, no cost, no severity.
#[test]
fn two_syntactically_identical_clones_are_recorded_without_ranking_them() {
    let fixture = load("scalar_and_aggregate_clone.rs");
    let clones = fixture.args(own::CLONE_SITE);
    assert_eq!(clones.len(), 2, "both clones observed: {clones:?}");
    assert_eq!(
        clones
            .iter()
            .map(|args| (args[1].as_str(), args[2].as_str()))
            .collect::<BTreeSet<_>>(),
        [("clone", "small"), ("clone", "big")]
            .into_iter()
            .collect::<BTreeSet<_>>(),
        "the operands distinguish them; nothing else does"
    );
    // Every edge about either site is at `ast` strength, which is the whole
    // mechanism that stops a size claim being made from syntax.
    for args in fixture.args(own::OWNERSHIP_EVIDENCE) {
        assert_eq!(
            args[1], "ast",
            "syntax cannot establish cost, so no site may be promoted"
        );
    }
}

// The two core `clone_before_await` shapes: a cloned local and a cloned struct
// field. Both must relate, and neither may claim more than order.
#[test]
fn a_clone_that_lexically_precedes_an_await_relates_to_it() {
    for (name, function, operand) in [
        ("clone_before_await.rs", "clone_then_await", "data"),
        (
            "clone_then_await_field.rs",
            "clone_field_then_await",
            "cfg.value",
        ),
    ] {
        let fixture = load(name);
        let relations = fixture.args(own::CLONE_BEFORE_AWAIT);
        assert_eq!(
            relations.len(),
            1,
            "{name}: one clone, one await, one relation: {relations:?}"
        );
        assert_eq!(
            short_name(&relations[0][0]),
            function,
            "{name}: the relation names the enclosing function"
        );
        let cloned = fixture
            .args(own::CLONE_SITE)
            .into_iter()
            .find(|args| args[0] == relations[0][1])
            .map(|args| args[2].clone())
            .unwrap_or_default();
        assert_eq!(
            cloned, operand,
            "{name}: the clone end names what was actually cloned"
        );
    }
}

// D1's UFCS row and §6.1's `Clone::clone`: the path-call form is a clone site
// anchored on its `scoped_identifier` callee, with the argument as the operand.
//
// Two documented absences are pinned alongside it, because both are silent:
//
// 1. `Iterator::filter(xs, p)` produces no filter site and therefore no
//    `filter_before_clone` — D2 declares UFCS iterator calls out of scope for
//    Phase One and names it as an incompleteness, not as cleanliness.
// 2. The *qualified* form `<Vec<i32> as Clone>::clone(&data)` produces no site
//    either. D1's grammar row covers only the plain `Clone::clone` spelling,
//    whose callee path is the bare identifier `Clone`; the qualified form's
//    path is a `bracketed_type`, which does not match, so §7.11's "emit no
//    edge" applies. The MANIFEST expected two clone sites here; that
//    expectation was not met by the code and the entry was corrected to record
//    the incompleteness. This assertion pins today's behaviour deliberately: if
//    the qualified form is ever handled, this test must fail so that the
//    decision and the MANIFEST are updated with it.
#[test]
fn the_ufcs_clone_call_is_a_site_while_the_qualified_and_filter_forms_are_not() {
    let fixture = load("ufcs_clone.rs");
    let clones = fixture.args(own::CLONE_SITE);
    assert_eq!(
        clones.len(),
        1,
        "only the plain `Clone::clone(..)` spelling is recognised today: {clones:?}"
    );
    assert_eq!(
        (clones[0][1].as_str(), clones[0][2].as_str()),
        ("Clone::clone", "&data"),
        "the operation is the callee path as written and the operand is its argument"
    );
    assert_eq!(
        fixture.span_text(&clones[0][0]),
        "Clone::clone(&data)",
        "the span covers the whole call, which is what a reader is shown"
    );
    assert!(
        fixture.source.contains("<Vec<i32> as Clone>::clone(&data)"),
        "the qualified spelling must be present, or its absence proves nothing"
    );
    assert!(
        fixture.args(own::FILTER_SITE).is_empty(),
        "a UFCS `Iterator::filter` is not observed (D2, a named incompleteness)"
    );
    assert!(
        fixture.args(own::FILTER_BEFORE_CLONE).is_empty(),
        "and it therefore relates to nothing — an absence, never a claim that \
         the chain is clean"
    );
}
