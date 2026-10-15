//! `phr-mcp graph query` through the real binary: bare names reach fully
//! qualified IDs, misses print the diagnostics the MCP tool returns, and
//! `--json` stdout stays machine-readable.

use std::path::Path;
use std::process::Command;

const GRAPH: &str = r##"{"p":"tested_by","a":["rust:demo::network::Net::fire_all_consequences","rust:demo::network::tests::fires"],"src":"src/network.rs"}
{"p":"tested_by","a":["rust:demo::agenda::annotate_consequences","rust:demo::agenda::tests::annotates"],"src":"src/agenda.rs"}
{"p":"imports","a":["rust:demo::agenda","rust:demo::wme"],"src":"src/agenda.rs"}
"##;

fn project() -> tempfile::TempDir {
    let d = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(d.path().join(".phronesis")).expect("mkdir");
    std::fs::write(d.path().join(".phronesis/graph.jsonl"), GRAPH).expect("write graph");
    d
}

/// (stdout, stderr) of `graph query <args>`.
fn query(dir: &Path, args: &[&str]) -> (String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .current_dir(dir)
        .args(["graph", "query"])
        .args(args)
        .output()
        .expect("run graph query");
    assert!(out.status.success(), "graph query exits 0: {out:?}");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn bare_and_qualified_names_reach_the_fully_qualified_id() {
    let d = project();
    for arg in [
        "fire_all_consequences",
        "Net::fire_all_consequences",
        "::fire_all_consequences",
        "rust:demo::network::Net::fire_all_consequences",
        "*::fire_all_consequences",
    ] {
        let (out, err) = query(d.path(), &["tested_by", arg]);
        assert!(out.contains("1 match(es)."), "`{arg}`: {out}");
        assert!(out.contains("tests::fires"), "`{arg}`: {out}");
        assert!(err.is_empty(), "a hit prints no hint for `{arg}`: {err}");
    }
    let (out, _) = query(d.path(), &["imports", "*", "demo::wme"]);
    assert!(out.contains("1 match(es)."), "{out}");
}

#[test]
fn a_split_segment_matches_nothing_and_says_why() {
    let d = project();
    // `consequences` ends both IDs but starts mid-segment in each.
    let (out, err) = query(d.path(), &["tested_by", "consequences"]);
    assert!(out.contains("0 match(es)."), "{out}");
    assert!(err.contains("hint: "), "a miss prints a hint: {err}");
    assert!(err.contains("Try glob '*consequences*'"), "{err}");
}

#[test]
fn a_miss_with_no_near_match_prints_sample_argument_shapes() {
    let d = project();
    let (out, err) = query(d.path(), &["tested_by", "no_such_fn"]);
    assert!(out.contains("0 match(es)."), "{out}");
    assert!(
        err.contains("hint: Relation 'tested_by' has 2 edges"),
        "{err}"
    );
    assert!(
        err.contains("hint: sample arguments: rust:demo::network::Net::fire_all_consequences"),
        "{err}"
    );
}

#[test]
fn json_output_stays_parseable_and_hint_free() {
    let d = project();
    let (out, err) = query(d.path(), &["tested_by", "no_such_fn", "--json"]);
    let v: serde_json::Value = serde_json::from_str(&out).expect("stdout is one JSON value");
    assert_eq!(v["total"], 0);
    assert!(err.is_empty(), "--json prints no hints: {err}");

    let (out, _) = query(d.path(), &["tested_by", "fire_all_consequences", "--json"]);
    let v: serde_json::Value = serde_json::from_str(&out).expect("stdout is one JSON value");
    assert_eq!(v["total"], 1);
}
