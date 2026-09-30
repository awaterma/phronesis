//! SwiftPM per-test collection (SPEC-coverage-evidence §2, Part I
//! decision 3): one isolated `swift test --enable-code-coverage --filter`
//! run per graph test, the merged `default.profdata` copied out before the
//! next run, and one `llvm-cov export -format=lcov` per snapshot, prepended
//! with `TN:<graph test id>` so the lcov-dir import joins store regions.

use std::path::Path;

use crate::coverage::language::swift::{runner_native_name, selector_from_test_id};
use crate::coverage::pytest::file_stem_for;
use crate::graph::model::Edge;

/// (graph test id, `swift test --filter` selector) per graph Swift test.
pub type CollectEntry = (String, String);

/// Collection entries from the graph's Swift `defines_test` edges; every
/// other relation or namespace is ignored.
pub fn collection_entries(edges: &[Edge]) -> Vec<CollectEntry> {
    edges
        .iter()
        .filter(|e| e.p == "defines_test" && e.a.len() == 2 && e.a[1].starts_with("swift:"))
        .filter_map(|e| selector_from_test_id(&e.a[1]).map(|sel| (e.a[1].clone(), sel)))
        .collect()
}

fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// The collection script a container with the Swift toolchain runs
/// (`phr-mcp coverage collect --tool swift-cov [--emit-script]`). One
/// isolated `swift test` per entry: the runner's merged `default.profdata`
/// is copied out before the next run replaces it, and llvm-cov turns the
/// snapshot into per-test lcov tagged with the graph test id.
pub fn collection_script(entries: &[CollectEntry], out_dir: &Path) -> String {
    let mut s = String::from("#!/bin/sh\nset -eu\nswift --version\nOUT=");
    s.push_str(&quote(&out_dir.to_string_lossy()));
    s.push_str(
        "\nmkdir -p \"$OUT\"\n\
        PROFDATA=$(swift test --show-codecov-path | xargs dirname)/default.profdata\n\
        LLVM_COV=$(xcrun -f llvm-cov 2>/dev/null || command -v llvm-cov)\n\
        BIN_PATH=$(swift build --show-bin-path)\n\
        set -- $(find \"$BIN_PATH\" -maxdepth 1 -name '*.xctest')\n\
        if [ $# -eq 0 ]; then\n\
        \x20   echo \"no *.xctest bundle under $BIN_PATH\" >&2\n\
        \x20   exit 1\n\
        fi\n\
        if [ $# -gt 1 ]; then\n\
        \x20   echo \"several *.xctest bundles under $BIN_PATH\" \"$@\" >&2\n\
        \x20   exit 1\n\
        fi\n\
        XCTEST_BIN=\"$1/Contents/MacOS/$(basename \"$1\" .xctest)\"\n\
        if [ ! -x \"$XCTEST_BIN\" ]; then\n\
        \x20   echo \"no executable in $1/Contents/MacOS\" >&2\n\
        \x20   exit 1\n\
        fi\n\
        n=0\n",
    );
    let mut stems = std::collections::BTreeMap::<String, usize>::new();
    for (index, (id, selector)) in entries.iter().enumerate() {
        let n = index + 1;
        let base = file_stem_for(id);
        let count = stems
            .entry(base.clone())
            .and_modify(|c| *c += 1)
            .or_insert(1);
        let stem = if *count > 1 {
            format!("{base}_{}", *count)
        } else {
            base
        };
        let native = runner_native_name(id).unwrap_or_default();
        s.push_str(&format!(
            "n=$((n+1))\n\
            swift test --enable-code-coverage --filter {sel} > \"$OUT/{n}.log\" 2>&1\n\
            grep -Eq 'Executed 1 test|Test run with 1 test' \"$OUT/{n}.log\" || {{ echo \"filter matched zero or several tests for {id}: see $OUT/{n}.log\" >&2; exit 1; }}\n\
            cp \"$PROFDATA\" \"$OUT/{n}.profdata\"\n\
            \"$LLVM_COV\" export -format=lcov -instr-profile \"$OUT/{n}.profdata\" \"$XCTEST_BIN\" > \"$OUT/{n}.raw.lcov\"\n\
            printf '%s\\n' {tn} {node} | cat - \"$OUT/{n}.raw.lcov\" > \"$OUT/{n}.tmp\" && mv \"$OUT/{n}.tmp\" \"$OUT/{stem}.lcov\"\n\
            rm -f \"$OUT/{n}.raw.lcov\"\n",
            sel = quote(selector),
            tn = quote(&format!("TN:{id}")),
            node = quote(&format!("# node: {native}")),
        ));
    }
    s.push_str("python3 - \"$OUT\" <<'PY'\nimport hashlib,json,os,subprocess,sys\nout=sys.argv[1]; root=subprocess.check_output(['git','rev-parse','--show-toplevel'],text=True).strip(); files={}\nfor name in sorted(os.listdir(out)):\n if name.endswith(('.lcov','.info')):\n  for line in open(os.path.join(out,name)):\n   if line.startswith('SF:'):\n    p=os.path.realpath(line[3:].strip()); rel=os.path.relpath(p,root)\n    if rel.startswith('..'+os.sep): raise SystemExit('source outside git root: '+p)\n    files[rel]=hashlib.sha256(open(p,'rb').read()).hexdigest()\nrev=subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip()\njson.dump({'revision':rev,'files':files},open(os.path.join(out,'manifest.json'),'w'),sort_keys=True)\nPY\n");
    s.push_str(&format!(
        "echo {}\n",
        quote(&format!(
            "phr-mcp coverage import --format lcov-dir --tool swift-cov {}",
            out_dir.display()
        ))
    ));
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::coverage::language::swift::selector_from_test_id;

    fn entry(id: &str) -> CollectEntry {
        (id.to_string(), selector_from_test_id(id).expect("selector"))
    }

    #[test]
    fn entries_come_from_swift_defines_test_edges_only() {
        let edges = vec![
            Edge {
                p: "defines_test".into(),
                a: vec![
                    "Tests/StoreKitTests/StoreTests.swift".into(),
                    "swift:store-kitTests::StoreTests::StoreTests::testLoad".into(),
                ],
                src: "Tests/StoreKitTests/StoreTests.swift".into(),
                d: false,
            },
            Edge {
                p: "defines_fn".into(),
                a: vec![
                    "Sources/Store/Store.swift".into(),
                    "swift:store-kit::Store::load".into(),
                ],
                src: "Sources/Store/Store.swift".into(),
                d: false,
            },
            Edge {
                p: "defines_test".into(),
                a: vec![
                    "tests/store.test.ts".into(),
                    "typescript:myapp::tests::store::Store loads".into(),
                ],
                src: "tests/store.test.ts".into(),
                d: false,
            },
        ];
        let entries = collection_entries(&edges);
        assert_eq!(
            entries,
            vec![(
                "swift:store-kitTests::StoreTests::StoreTests::testLoad".to_string(),
                "^store_kitTests\\.StoreTests/testLoad$".to_string()
            )],
            "{entries:?}"
        );
    }

    #[test]
    fn script_runs_one_isolated_swift_test_per_entry_and_exports_lcov() {
        let entries = vec![entry("swift:store-kitTests::StoreTests::StoreTests::testLoad")];
        let s = collection_script(&entries, Path::new("/tmp/cov"));
        assert!(
            s.contains(
                "swift test --enable-code-coverage --filter '^store_kitTests\\.StoreTests/testLoad$' > \"$OUT/1.log\" 2>&1"
            ),
            "{s}"
        );
        assert!(
            s.contains("grep -Eq 'Executed 1 test|Test run with 1 test' \"$OUT/1.log\""),
            "the zero-match guard must accept both dialects: {s}"
        );
        assert!(s.contains("cp \"$PROFDATA\" \"$OUT/1.profdata\""), "{s}");
        assert!(
            s.contains(
                "\"$LLVM_COV\" export -format=lcov -instr-profile \"$OUT/1.profdata\" \"$XCTEST_BIN\" > \"$OUT/1.raw.lcov\""
            ),
            "{s}"
        );
        assert!(
            s.contains("TN:swift:store-kitTests::StoreTests::StoreTests::testLoad"),
            "{s}"
        );
        assert!(s.contains("# node: store_kitTests.StoreTests/testLoad"), "{s}");
        assert!(s.contains("manifest.json") && s.contains("rev-parse"), "{s}");
        assert!(
            s.contains("phr-mcp coverage import --format lcov-dir --tool swift-cov"),
            "{s}"
        );
    }

    #[test]
    fn script_resolves_profdata_llvm_cov_and_exactly_one_xctest_bundle() {
        let s = collection_script(
            &[entry("swift:store-kitTests::StoreTests::StoreTests::testLoad")],
            Path::new("/tmp/cov"),
        );
        assert!(
            s.contains("PROFDATA=$(swift test --show-codecov-path | xargs dirname)/default.profdata"),
            "{s}"
        );
        assert!(
            s.contains("LLVM_COV=$(xcrun -f llvm-cov 2>/dev/null || command -v llvm-cov)"),
            "{s}"
        );
        assert!(
            s.contains("find \"$BIN_PATH\" -maxdepth 1 -name '*.xctest'"),
            "{s}"
        );
        assert!(s.contains("no *.xctest bundle under"), "{s}");
        assert!(s.contains("several *.xctest bundles under"), "{s}");
        assert!(s.contains("Contents/MacOS"), "the bundle's executable: {s}");
        assert!(s.contains("set -eu"), "{s}");
    }
}