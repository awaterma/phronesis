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
        find \"$OUT\" -maxdepth 1 -type f \\( -name '*.lcov' -o -name '*.info' -o -name '*.profdata' -o -name 'manifest.json' \\) -delete\n\
        swift build --build-tests --enable-code-coverage\n\
        PROFDATA=$(dirname \"$(swift test --show-codecov-path)\")/default.profdata\n\
        LLVM_COV=$(xcrun -f llvm-cov 2>/dev/null || command -v llvm-cov)\n\
        BIN_PATH=$(swift build --show-bin-path)\n\
        set -- \"$BIN_PATH\"/*.xctest\n\
        if [ ! -e \"$1\" ]; then\n\
        \x20   echo \"no *.xctest bundle under $BIN_PATH\" >&2\n\
        \x20   exit 1\n\
        fi\n\
        if [ $# -gt 1 ]; then\n\
        \x20   echo \"several *.xctest bundles under $BIN_PATH\" \"$@\" >&2\n\
        \x20   exit 1\n\
        fi\n\
        if [ -d \"$1\" ]; then\n\
        \x20   XCTEST_BIN=\"$1/Contents/MacOS/$(basename \"$1\" .xctest)\"\n\
        else\n\
        \x20   XCTEST_BIN=\"$1\"\n\
        fi\n\
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
            rm -f \"$PROFDATA\"\n\
            swift test --enable-code-coverage --filter {sel} > \"$OUT/{n}.log\" 2>&1\n\
            python3 - \"$OUT/{n}.log\" <<'COUNT'\n\
            import re,sys\n\
            text=open(sys.argv[1]).read()\n\
            x=max([int(n) for n in re.findall(r'Executed ([0-9]+) tests?\\b',text)]+[0])\n\
            t=max([int(n) for n in re.findall(r'Test run with ([0-9]+) tests?\\b',text)]+[0])\n\
            skipped=max([int(n) for n in re.findall(r'([0-9]+) tests? skipped',text)]+[0])\n\
            if x+t != 1 or skipped: raise SystemExit('filter must execute exactly one test: '+sys.argv[1])\n\
            COUNT\n\
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
        let entries = vec![entry(
            "swift:store-kitTests::StoreTests::StoreTests::testLoad",
        )];
        let s = collection_script(&entries, Path::new("/tmp/cov"));
        assert!(
            s.contains(
                "swift test --enable-code-coverage --filter '^store_kitTests\\.StoreTests/testLoad$' > \"$OUT/1.log\" 2>&1"
            ),
            "{s}"
        );
        assert!(s.contains("if x+t != 1 or skipped"));
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
        assert!(
            s.contains("# node: store_kitTests.StoreTests/testLoad"),
            "{s}"
        );
        assert!(
            s.contains("manifest.json") && s.contains("rev-parse"),
            "{s}"
        );
        assert!(
            s.contains("phr-mcp coverage import --format lcov-dir --tool swift-cov"),
            "{s}"
        );
    }

    #[test]
    fn script_resolves_profdata_llvm_cov_and_exactly_one_xctest_bundle() {
        let s = collection_script(
            &[entry(
                "swift:store-kitTests::StoreTests::StoreTests::testLoad",
            )],
            Path::new("/tmp/cov"),
        );
        assert!(
            s.contains(
                "PROFDATA=$(dirname \"$(swift test --show-codecov-path)\")/default.profdata"
            ),
            "{s}"
        );
        assert!(
            s.contains("LLVM_COV=$(xcrun -f llvm-cov 2>/dev/null || command -v llvm-cov)"),
            "{s}"
        );
        assert!(s.contains("set -- \"$BIN_PATH\"/*.xctest"), "{s}");
        assert!(s.contains("no *.xctest bundle under"), "{s}");
        assert!(s.contains("several *.xctest bundles under"), "{s}");
        assert!(s.contains("Contents/MacOS"), "the bundle's executable: {s}");
        assert!(s.contains("set -eu"), "{s}");
    }

    #[cfg(unix)]
    #[test]
    fn generated_script_builds_clean_layouts_rejects_bad_counts_and_clears_stale_output() {
        use std::os::unix::fs::PermissionsExt;
        for layout in ["bundle", "file"] {
            let dir = tempfile::tempdir().expect("tempdir");
            let root_path = dir.path().join("project with spaces");
            std::fs::create_dir(&root_path).expect("project");
            let root = root_path.as_path();
            let bin = root.join("tools");
            std::fs::create_dir(&bin).expect("tools");
            let write_tool = |name: &str, text: &str| {
                let p = bin.join(name);
                std::fs::write(&p, text).expect("write tool");
                std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).expect("chmod");
            };
            write_tool(
                "swift",
                r#"#!/bin/sh
set -eu
case "$*" in
 --version) echo Swift ;;
 'build --build-tests --enable-code-coverage')
 mkdir -p "$ROOT/build"
 if [ "$LAYOUT" = bundle ]; then
 mkdir -p "$ROOT/build/Example.xctest/Contents/MacOS"
 touch "$ROOT/build/Example.xctest/Contents/MacOS/Example"
 chmod +x "$ROOT/build/Example.xctest/Contents/MacOS/Example"
 else
 touch "$ROOT/build/Example.xctest"; chmod +x "$ROOT/build/Example.xctest"
 fi ;;
 'build --show-bin-path') echo "$ROOT/build" ;;
 'test --show-codecov-path') echo "$ROOT/build/coverage.json" ;;
 test*)
 test ! -f "$ROOT/build/default.profdata"
 echo fresh > "$ROOT/build/default.profdata"
 printf '%s\n' "$SUMMARY" ;;
 *) exit 9 ;;
esac
"#,
            );
            write_tool("xcrun", "#!/bin/sh\nexit 1\n");
            write_tool(
                "llvm-cov",
                "#!/bin/sh\ntest -x \"$5\" || exit 9\necho TN:\necho end_of_record\n",
            );
            write_tool(
                "git",
                "#!/bin/sh\ncase \"$*\" in\n 'rev-parse --show-toplevel') echo \"$ROOT\" ;;\n *) echo abc ;;\nesac\n",
            );
            let out = root.join("output");
            let script = root.join("collect.sh");
            std::fs::write(
                &script,
                collection_script(
                    &[entry(
                        "swift:exampleTests::StoreTests::StoreTests::testLoad",
                    )],
                    &out,
                ),
            )
            .expect("script");
            let run = |summary: &str| {
                std::process::Command::new("sh")
                    .arg(&script)
                    .env("ROOT", root)
                    .env("LAYOUT", layout)
                    .env("SUMMARY", summary)
                    .env(
                        "PATH",
                        format!("{}:{}", bin.display(), std::env::var("PATH").expect("PATH")),
                    )
                    .output()
                    .expect("run script")
            };
            let result = run("Executed 1 test, with 0 failures");
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
            std::fs::write(out.join("stale.lcov"), "stale").expect("stale");
            assert!(run("Test run with 1 test passed").status.success());
            assert!(!out.join("stale.lcov").exists());
            for summary in [
                "Executed 0 tests",
                "Executed 1 test, with 1 test skipped and 0 failures",
                "Executed 11 tests",
                "Executed 21 tests",
                "Executed 1 test\nExecuted 2 tests",
                "Executed 1 test\nTest run with 1 test passed",
            ] {
                let result = run(summary);
                assert!(!result.status.success(), "accepted {summary}");
                assert!(!out.join("manifest.json").exists());
            }
        }
    }

    #[test]
    #[ignore = "requires a Swift toolchain and llvm-cov; run explicitly on Swift CI"]
    fn real_swiftpm_clean_checkout_collects_one_test_and_reruns() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        std::fs::create_dir_all(root.join("Sources/Example")).expect("sources");
        std::fs::create_dir_all(root.join("Tests/ExampleTests")).expect("tests");
        std::fs::write(root.join("Package.swift"), "// swift-tools-version: 5.9\nimport PackageDescription\nlet package = Package(name: \"Example\", products: [.library(name: \"Example\", targets: [\"Example\"])], targets: [.target(name: \"Example\"), .testTarget(name: \"ExampleTests\", dependencies: [\"Example\"])])\n").expect("package");
        std::fs::write(
            root.join("Sources/Example/Value.swift"),
            "public func value() -> Int { 7 }\n",
        )
        .expect("source");
        std::fs::write(root.join("Tests/ExampleTests/ValueTests.swift"), "import XCTest\n@testable import Example\nfinal class ValueTests: XCTestCase {\nfunc testValue() { XCTAssertEqual(Example.value(), 7) }\nfunc testValueAgain() { XCTAssertEqual(Example.value(), 7) }\nfunc testSkipped() throws { throw XCTSkip(\"fixture\") }\n}\n").expect("tests");
        for args in [
            vec!["init", "-q"],
            vec!["add", "Package.swift", "Sources", "Tests"],
            vec![
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "-qm",
                "fixture",
            ],
        ] {
            assert!(
                std::process::Command::new("git")
                    .args(args)
                    .current_dir(root)
                    .status()
                    .expect("git")
                    .success()
            );
        }
        let out = root.join("coverage");
        let script = root.join("collect.sh");
        std::fs::write(
            &script,
            collection_script(
                &[entry(
                    "swift:ExampleTests::ValueTests::ValueTests::testValue",
                )],
                &out,
            ),
        )
        .expect("script");
        for _ in 0..2 {
            let result = std::process::Command::new("sh")
                .arg(&script)
                .current_dir(root)
                .output()
                .expect("collect");
            assert!(
                result.status.success(),
                "stdout={} stderr={}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
            let reports: Vec<_> = std::fs::read_dir(&out)
                .expect("output")
                .map(|e| e.expect("entry").path())
                .filter(|p| p.extension().is_some_and(|e| e == "lcov"))
                .collect();
            assert_eq!(reports.len(), 1);
            let report = std::fs::read_to_string(&reports[0]).expect("lcov");
            assert!(report.contains("TN:swift:ExampleTests::ValueTests::ValueTests::testValue"));
            assert!(report.contains("Sources/Example/Value.swift"));
            assert!(report.contains("DA:1,1"), "{report}");
            assert!(out.join("manifest.json").exists());
            std::fs::write(out.join("stale.lcov"), "stale").expect("stale");
        }
        std::fs::write(
            &script,
            collection_script(
                &[entry(
                    "swift:ExampleTests::ValueTests::ValueTests::testSkipped",
                )],
                &out,
            ),
        )
        .expect("skip script");
        let result = std::process::Command::new("sh")
            .arg(&script)
            .current_dir(root)
            .output()
            .expect("collect skipped test");
        assert!(
            !result.status.success(),
            "skipped test must not acquire coverage"
        );
        assert!(
            String::from_utf8_lossy(&result.stderr)
                .contains("filter must execute exactly one test")
        );
        assert!(!out.join("manifest.json").exists());
    }
}
