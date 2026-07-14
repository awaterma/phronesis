//! Bench journey-fact derivation across a sweep of suffix sizes.
//!
//! Sweep `N ∈ {1k, 5k, 10k, 25k, 50k, 100k}` events in the journal suffix.
//! Today `SUFFIX_HARD_CAP = 10_000` clamps `read_recent`, so the points past
//! 10k characterize the curve we'd land on if the cap is raised. The cap is
//! defensive (see SPEC-journey-facts §"Cost"), not a measured cliff —
//! this bench gives the data to size that decision honestly.
//!
//! Setup writes a scratch project under `target/<tmpdir>/journey-bench/<n>/`
//! containing only `.phronesis/journey/events.jsonl`. Rules and tagger config
//! are passed in-process to `derive::assert_facts`, so no `rules.json` or
//! `journey.json` needs to exist on disk.
//!
//! Run: `cargo bench -p phronesis-mcp --bench journey_derive`

use std::path::{Path, PathBuf};

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use phr::{Action, Condition, ReteNetwork, Rule};
use phronesis_mcp::journey::{
    derive,
    journal::JournalRecord,
    tagger::{TaggerConfig, TaggerEntry},
};
use rand::SeedableRng;
use rand::rngs::StdRng;
use rand::seq::SliceRandom;
use tokio::runtime::{Builder, Runtime};

const SIZES: &[usize] = &[1_000, 5_000, 10_000, 25_000, 50_000, 100_000];
const SEED: u64 = 0x5eed_5eed_5eed_5eed;
const BASE_TS: u64 = 1_781_911_122;
const SID: &str = "s-bench";
const TAG: &str = "bench_tag";

fn build_runtime() -> Runtime {
    Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime")
}

/// Read the in-repo seed corpus shipped with `phronesis-mcp`. We replay-and-scale
/// from real records so synthetic events traverse the same deserializer +
/// matcher paths as production journal entries.
fn load_seed_records() -> Vec<JournalRecord> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(".phronesis/journey/events.jsonl");
    let content = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read seed corpus {}: {}", path.display(), e));
    let records: Vec<JournalRecord> = content
        .lines()
        .filter_map(|l| serde_json::from_str::<JournalRecord>(l).ok())
        .collect();
    assert!(!records.is_empty(), "seed corpus is empty");
    records
}

/// Sample-with-replacement from the seed corpus to size N, re-stamping
/// monotonic `seq` and `ts` and overwriting tags so every record matches the
/// bench rule's selector. Seeded RNG → same `(seed, n)` always produces the
/// same bytes.
fn synth_records(seed_corpus: &[JournalRecord], n: usize) -> Vec<JournalRecord> {
    let mut rng = StdRng::seed_from_u64(SEED);
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let src = seed_corpus.choose(&mut rng).expect("non-empty seed");
        let mut rec = src.clone();
        rec.seq = (i as u64) + 1;
        rec.ts = BASE_TS + i as u64;
        rec.sid = SID.to_string();
        rec.tags = vec![TAG.to_string()];
        out.push(rec);
    }
    out
}

fn write_scratch_project(root: &Path, records: &[JournalRecord]) -> std::io::Result<()> {
    let journey_dir = root.join(".phronesis").join("journey");
    std::fs::create_dir_all(&journey_dir)?;
    let mut body = String::with_capacity(records.len() * 128);
    for rec in records {
        body.push_str(&serde_json::to_string(rec).expect("serialize record"));
        body.push('\n');
    }
    std::fs::write(journey_dir.join("events.jsonl"), body)
}

/// One rule per aggregator family that exercises the bench tag. Each rule's
/// `__script__` body names a `(predicate, selector, window)` triple the
/// derivation scan picks up — so derivation walks the full suffix for every
/// family on every pass.
fn build_rules() -> Vec<Rule> {
    let mk = |id: &str, script: String| Rule {
        id: id.into(),
        priority: 0,
        conditions: vec![Condition {
            predicate: "__script__".into(),
            args: vec![],
            script: Some(script),
        }],
        actions: vec![Action {
            action_type: "log".into(),
            params: vec!["hit".into()],
        }],
    };
    vec![
        mk(
            "b-occurrence-session",
            format!("facts_count('journey_occurrence', ['{TAG}','s']) >= 1"),
        ),
        mk(
            "b-count-session",
            format!("facts_count('journey_count', ['{TAG}','s']) >= 1"),
        ),
        mk(
            "b-seen-session",
            format!("facts_count('journey_seen', ['{TAG}','s']) >= 1"),
        ),
        // Calls-window flavor so `max_call_window` is non-zero too.
        mk(
            "b-occurrence-calls",
            format!("facts_count('journey_occurrence', ['{TAG}','100c']) >= 1"),
        ),
    ]
}

fn bench_journey_derive(c: &mut Criterion) {
    let rt = build_runtime();
    let seed = load_seed_records();
    let rules = build_rules();
    // Register the bench selector so `validate_selectors` accepts our rules.
    // The tagger's `when` is irrelevant here — we pre-stamp tags directly on
    // records, bypassing tagger evaluation.
    let mut cfg = TaggerConfig::default();
    cfg.taggers.push(TaggerEntry {
        tag: TAG.into(),
        when: vec![],
    });
    let scratch_root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("journey-bench");

    let mut group = c.benchmark_group("journey_derive");
    // 100k point is the slowest; cap sample count so wall time stays bounded.
    group.sample_size(15);

    let mut scratches: Vec<(usize, PathBuf)> = Vec::with_capacity(SIZES.len());
    for &n in SIZES {
        let scratch = scratch_root.join(n.to_string());
        let records = synth_records(&seed, n);
        write_scratch_project(&scratch, &records).expect("write scratch project");
        scratches.push((n, scratch));
    }

    for (n, scratch) in &scratches {
        group.throughput(Throughput::Elements(*n as u64));
        let now_ts = BASE_TS + (*n as u64) + 1;
        group.bench_with_input(BenchmarkId::from_parameter(n), n, |b, _| {
            b.to_async(&rt).iter(|| async {
                let mut net = ReteNetwork::new();
                derive::assert_facts(
                    &mut net,
                    derive::DeriveInput {
                        project_root: scratch,
                        rules: &rules,
                        config: &cfg,
                        scope: derive::WindowScope {
                            current_sid: SID,
                            now_ts,
                        },
                    },
                )
                .await
                .expect("derive::assert_facts");
            });
        });
    }

    group.finish();
}

/// Full per-turn path: register rules + derive + fire. Mirrors what
/// `pre-check` / `post-check` do, so per-turn user-perceived latency is what
/// this group measures. Compare against `journey_derive` to isolate the
/// firing share of the cost — derivation alone is in `journey_derive`,
/// `(full - derive) ≈ rule firing under the resulting WM pressure`.
fn bench_journey_full(c: &mut Criterion) {
    let rt = build_runtime();
    let seed = load_seed_records();
    let rules = build_rules();
    let mut cfg = TaggerConfig::default();
    cfg.taggers.push(TaggerEntry {
        tag: TAG.into(),
        when: vec![],
    });
    let scratch_root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("journey-bench");

    let mut group = c.benchmark_group("journey_full");
    group.sample_size(15);

    let mut scratches: Vec<(usize, PathBuf)> = Vec::with_capacity(SIZES.len());
    for &n in SIZES {
        let scratch = scratch_root.join(n.to_string());
        let records = synth_records(&seed, n);
        write_scratch_project(&scratch, &records).expect("write scratch project");
        scratches.push((n, scratch));
    }

    for (n, scratch) in &scratches {
        group.throughput(Throughput::Elements(*n as u64));
        let now_ts = BASE_TS + (*n as u64) + 1;
        group.bench_with_input(BenchmarkId::from_parameter(n), n, |b, _| {
            b.to_async(&rt).iter(|| async {
                let mut net = ReteNetwork::new();
                for rule in &rules {
                    net.add_rule(rule.clone()).await.expect("add_rule");
                }
                derive::assert_facts(
                    &mut net,
                    derive::DeriveInput {
                        project_root: scratch,
                        rules: &rules,
                        config: &cfg,
                        scope: derive::WindowScope {
                            current_sid: SID,
                            now_ts,
                        },
                    },
                )
                .await
                .expect("derive::assert_facts");
                let _ = net.fire_all_consequences().expect("fire_all_consequences");
            });
        });
    }

    group.finish();
}

criterion_group!(benches, bench_journey_derive, bench_journey_full);
criterion_main!(benches);
