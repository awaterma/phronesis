//! Bench the two latency gates of the structural graph feature.
//!
//! Spec `docs/specs/SPEC-triple-store-rete.md` §8 makes these acceptance
//! gates rather than assumptions:
//!
//! * **per-save** (`PostToolUse`): parse one file, compact by provenance,
//!   re-derive over the whole graph, write atomically. Scaling here is driven
//!   by the *derive* tier, which is linear in total edges, not by the parse
//!   tier, which sees exactly one file.
//! * **hydrate** (`PreToolUse`): load the graph and turn the requested
//!   relations into facts.
//!
//! Both sweep graph size so the curve — not just the point at today's repo
//! size — is on record. The spec's working estimate is ~15k edges for a
//! 1,000-file codebase.
//!
//! Run: `cargo bench -p phronesis-mcp --bench graph_sync`

use std::path::{Path, PathBuf};

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use phr::{Condition, Rule};
use phronesis_mcp::graph::{hydrate, model::Edge, store, sync};

/// Edge counts to sweep. 15k is the spec's estimate for a 1,000-file project;
/// 30k characterizes the curve past it.
const SIZES: &[usize] = &[1_000, 5_000, 15_000, 30_000];

/// One source file's worth of content, used for the parse tier.
const SAMPLE_FILE: &str = r#"
use crate::other::Thing;

pub struct Widget;

impl Widget {
    pub fn build(&self) -> Option<Thing> {
        let t = self.lookup()?;
        Some(t)
    }

    fn lookup(&self) -> Option<Thing> {
        None
    }
}

pub fn helper(input: &str) -> String {
    input.to_string()
}
"#;

/// Build a synthetic graph of roughly `n` base edges spread across files, with
/// a realistic mix of relations. Coverage is left partial so the `untested`
/// set difference has real work to do.
fn synth_edges(n: usize) -> Vec<Edge> {
    let mut edges = Vec::with_capacity(n);
    let mut i = 0usize;
    while edges.len() < n {
        let file = format!("src/m{}.rs", i / 10);
        let func = format!("crate::m{}::f{}", i / 10, i);
        edges.push(Edge::base("file_type", &[&file, "production"], &file));
        edges.push(Edge::base("defines_fn", &[&file, &func], &file));
        if i.is_multiple_of(3) {
            edges.push(Edge::base("calls_api", &[&func, "unwrap"], &file));
        }
        // Two thirds of functions have a direct test.
        if i % 3 != 2 {
            let test = format!("crate::tests::t{i}");
            edges.push(Edge::base("tested_by", &[&func, &test], "tests/all.rs"));
        }
        // A sparse import chain, mostly acyclic with occasional back-edges so
        // Tarjan finds real SCCs rather than degenerating to singletons.
        let from = format!("crate::m{}", i / 10);
        let to = format!("crate::m{}", (i / 10 + 1) % (n / 10 + 1));
        edges.push(Edge::base("imports", &[&from, &to], &file));
        i += 1;
    }
    edges.truncate(n);
    edges
}

/// Materialize a scratch project containing a graph of `n` edges.
fn setup_project(root: &Path, n: usize) {
    std::fs::create_dir_all(root.join("src")).expect("mkdir src");
    std::fs::write(root.join("src/subject.rs"), SAMPLE_FILE).expect("write subject");
    store::write_atomic(&store::graph_path(root), &synth_edges(n)).expect("seed graph");
}

fn scratch_dir(n: usize) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/graph-bench")
        .join(n.to_string());
    if dir.exists() {
        std::fs::remove_dir_all(&dir).expect("clean scratch");
    }
    std::fs::create_dir_all(&dir).expect("mkdir scratch");
    dir
}

fn rule_using(predicate: &str) -> Rule {
    Rule {
        id: format!("uses-{predicate}"),
        priority: 0,
        conditions: vec![Condition {
            predicate: predicate.to_string(),
            args: vec!["?a".into(), "?b".into()],
            script: None,
        }],
        actions: vec![],
    }
}

/// PostToolUse: the full per-save round trip.
fn bench_per_save(c: &mut Criterion) {
    let mut group = c.benchmark_group("graph/per_save");
    for &n in SIZES {
        let root = scratch_dir(n);
        setup_project(&root, n);
        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.iter(|| {
                sync::on_save(&root, "src/subject.rs", SAMPLE_FILE).expect("save");
            });
        });
    }
    group.finish();
}

/// PreToolUse: load the graph and produce facts for the rules that need them.
fn bench_hydrate(c: &mut Criterion) {
    let rules = vec![
        rule_using("defines_fn"),
        rule_using("calls_api"),
        rule_using("untested"),
        rule_using("in_cycle"),
    ];
    let mut group = c.benchmark_group("graph/hydrate");
    for &n in SIZES {
        let root = scratch_dir(n);
        setup_project(&root, n);
        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::from_parameter(n), &n, |b, _| {
            b.iter(|| {
                let h = hydrate::hydrate(&root, &rules);
                std::hint::black_box(h.facts.len());
            });
        });
    }
    group.finish();
}

/// The zero-cost path: a project whose rules never mention a graph relation
/// must not pay for the graph at all. This is the number that protects every
/// existing project's hook latency.
fn bench_hydrate_unused(c: &mut Criterion) {
    let rules = vec![rule_using("file_path")];
    let root = scratch_dir(15_000);
    setup_project(&root, 15_000);
    c.bench_function("graph/hydrate_unused_15000", |b| {
        b.iter(|| {
            let h = hydrate::hydrate(&root, &rules);
            std::hint::black_box(h.facts.len());
        });
    });
}

criterion_group!(benches, bench_per_save, bench_hydrate, bench_hydrate_unused);
criterion_main!(benches);
