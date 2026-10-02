//! CLI import rejection must reach the intended parser/mapping path and preserve evidence.
use phronesis_mcp::properties::execute::artifact_sha256;
use std::{
    path::Path,
    process::{Command, Output},
};

struct Fixture {
    temp: tempfile::TempDir,
    format: &'static str,
    tool: &'static str,
}
impl Fixture {
    fn new(format: &'static str) -> Self {
        let temp = tempfile::tempdir().expect("fixture");
        let root = temp.path();
        for file in ["pkg/store.py", "other/pkg/store.py"] {
            write(root, file, "def load():\n    return 7\n");
        }
        for file in [
            "core/src/main/java/com/x/Store.java",
            "other/src/main/java/com/x/Store.java",
        ] {
            write(
                root,
                file,
                "package com.x;\npublic class Store {\n public int load() {\n  return 7;\n }\n}\n",
            );
        }
        write(root, ".gitignore", ".phronesis/\n");
        for args in [
            vec!["init", "-q"],
            vec!["add", "."],
            vec![
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "commit",
                "-qm",
                "fixture",
            ],
        ] {
            assert!(
                Command::new("git")
                    .args(args)
                    .current_dir(root)
                    .status()
                    .expect("git")
                    .success()
            );
        }
        write(root, ".phronesis/rules.json", r#"{"rules":[]}"#);
        let revision = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(root)
            .output()
            .expect("revision");
        let mut files = serde_json::Map::new();
        for file in [
            "pkg/store.py",
            "other/pkg/store.py",
            "core/src/main/java/com/x/Store.java",
            "other/src/main/java/com/x/Store.java",
        ] {
            files.insert(
                file.into(),
                artifact_sha256(&std::fs::read(root.join(file)).expect("source")).into(),
            );
        }
        write(root, ".phronesis/reports/manifest.json", &serde_json::json!({"revision":String::from_utf8(revision.stdout).expect("UTF8").trim(), "files":files}).to_string());
        let fixture = Self {
            temp,
            format,
            tool: if format == "lcov-dir" {
                "coverage.py"
            } else {
                "jacoco+mvn"
            },
        };
        if format == "lcov-dir" {
            fixture.report("TN:python:project::tests::test_store::test_load\nSF:pkg/store.py\nDA:2,1\nend_of_record\n");
        } else {
            write(fixture.root(), ".phronesis/reports/1/module.txt", "core");
            write(
                fixture.root(),
                ".phronesis/reports/1/TN",
                "java:project::com::x::StoreTest::testLoad",
            );
            fixture.report(&jacoco("4", "1", None));
        }
        let first = fixture.import();
        assert!(
            first.status.success(),
            "baseline must reach successful verified import: {first:?}"
        );
        assert_eq!(fixture.records().len(), 1);
        fixture
    }
    fn root(&self) -> &Path {
        self.temp.path()
    }
    fn report(&self, text: &str) {
        write(
            self.root(),
            if self.format == "lcov-dir" {
                ".phronesis/reports/report.lcov"
            } else {
                ".phronesis/reports/1/jacoco.xml"
            },
            text,
        );
    }
    fn import(&self) -> Output {
        Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
            .current_dir(self.root())
            .args([
                "coverage",
                "import",
                "--format",
                self.format,
                "--tool",
                self.tool,
                ".phronesis/reports",
            ])
            .output()
            .expect("CLI")
    }
    fn evidence(&self) -> Vec<Vec<u8>> {
        ["coverage.jsonl", "coverage.index"]
            .iter()
            .map(|file| {
                std::fs::read(self.root().join(".phronesis").join(file)).expect("prior evidence")
            })
            .collect()
    }
    fn records(&self) -> Vec<serde_json::Value> {
        std::fs::read_to_string(self.root().join(".phronesis/coverage.jsonl"))
            .expect("store")
            .lines()
            .map(|line| serde_json::from_str(line).expect("record"))
            .collect()
    }
    fn reject_preserving(&self, expected_error: &str) {
        let before = self.evidence();
        let result = self.import();
        assert!(
            !result.status.success(),
            "accepted rejected input: {result:?}"
        );
        let error = String::from_utf8_lossy(&result.stderr);
        assert!(
            error.contains(expected_error),
            "wrong rejection path (wanted {expected_error:?}): {error}"
        );
        assert!(
            !error.contains("manifest"),
            "test must not stop at manifest validation: {error}"
        );
        assert_eq!(
            self.evidence(),
            before,
            "rejected import changed store or index"
        );
    }
}
fn write(root: &Path, file: &str, text: &str) {
    let path = root.join(file);
    std::fs::create_dir_all(path.parent().expect("parent")).expect("directory");
    std::fs::write(path, text).expect("file");
}
fn jacoco(line: &str, count: &str, covered: Option<&str>) -> String {
    let method = covered.map(|n| format!(r#"<class name="com/x/Store" sourcefilename="Store.java"><method name="load"><counter type="METHOD" missed="0" covered="{n}"/></method></class>"#)).unwrap_or_default();
    format!(
        r#"<report><package name="com/x">{method}<sourcefile name="Store.java"><line nr="{line}" mi="0" ci="{count}"/></sourcefile></package></report>"#
    )
}

#[test]
fn lcov_malformed_numeric_fields_reach_parser_and_preserve_verified_store_and_index() {
    let fixture = Fixture::new("lcov-dir");
    for value in ["-1", "1.5", "invalid", "18446744073709551616", ""] {
        for counter in [
            format!("DA:{value},1"),
            format!("DA:2,{value}"),
            format!("FNDA:{value},load\nDA:2,1"),
        ] {
            fixture.report(&format!("TN:python:project::tests::test_store::test_load\nSF:pkg/store.py\n{counter}\nend_of_record\n"));
            fixture.reject_preserving(if value == "18446744073709551616" {
                "number too large"
            } else if value.is_empty() {
                "empty string"
            } else {
                "invalid digit"
            });
        }
    }
}
#[test]
fn jacoco_malformed_numeric_fields_reach_parser_and_preserve_verified_store_and_index() {
    let fixture = Fixture::new("jacoco-dir");
    for value in ["-1", "1.5", "invalid", "18446744073709551616", ""] {
        for report in [
            jacoco(value, "1", None),
            jacoco("4", value, None),
            jacoco("4", "1", Some(value)),
        ] {
            fixture.report(&report);
            fixture.reject_preserving(if value == "18446744073709551616" {
                "number too large"
            } else if value.is_empty() {
                "empty string"
            } else {
                "invalid digit"
            });
        }
    }
}
#[test]
fn lcov_all_missing_or_ambiguous_sources_never_replace_prior_evidence() {
    let fixture = Fixture::new("lcov-dir");
    fixture.report(
        "TN:python:project::tests::test_store::test_load\nSF:missing.py\nDA:2,1\nend_of_record\n",
    );
    fixture.reject_preserving("1 unresolved");
    fixture.report("TN:python:project::tests::test_store::test_load\nSF:/producer/pkg/store.py\nDA:2,1\nend_of_record\n");
    fixture.reject_preserving("1 ambiguous");
}
#[test]
fn jacoco_all_missing_or_ambiguous_sources_never_replace_prior_evidence() {
    let fixture = Fixture::new("jacoco-dir");
    fixture.report(r#"<report><package name="com/x"><sourcefile name="Missing.java"><line nr="4" ci="1"/></sourcefile></package></report>"#);
    fixture.reject_preserving("no coverage records");
    fixture.report(&jacoco("4", "1", None));
    write(fixture.root(), ".phronesis/reports/1/module.txt", "absent");
    fixture.reject_preserving("no coverage records");
}
#[test]
fn lcov_mixed_resolved_missing_and_ambiguous_sources_import_only_grounded_hits() {
    let fixture = Fixture::new("lcov-dir");
    fixture.report("TN:python:project::tests::test_store::test_load\nSF:pkg/store.py\nDA:2,1\nend_of_record\nSF:missing.py\nDA:2,1\nend_of_record\nSF:/producer/pkg/store.py\nDA:2,1\nend_of_record\n");
    let result = fixture.import();
    assert!(result.status.success(), "{result:?}");
    assert!(
        String::from_utf8_lossy(&result.stdout).contains("1 unresolved SF, 1 ambiguous SF"),
        "{result:?}"
    );
    let records = fixture.records();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["file"], "pkg/store.py");
    assert_eq!(records[0]["region"], "fn:pkg/store.py::load");
}
#[test]
fn jacoco_mixed_resolved_missing_and_ambiguous_sources_import_only_grounded_hits() {
    let fixture = Fixture::new("jacoco-dir");
    fixture.report(r#"<report><package name="com/x"><sourcefile name="Store.java"><line nr="4" ci="1"/></sourcefile><sourcefile name="Missing.java"><line nr="4" ci="1"/></sourcefile></package></report>"#);
    write(fixture.root(), ".phronesis/reports/2/module.txt", "absent");
    write(
        fixture.root(),
        ".phronesis/reports/2/TN",
        "java:project::com::x::StoreTest::testUnresolved",
    );
    write(
        fixture.root(),
        ".phronesis/reports/2/jacoco.xml",
        &jacoco("4", "1", None),
    );
    let result = fixture.import();
    assert!(result.status.success(), "{result:?}");
    assert!(
        String::from_utf8_lossy(&result.stderr).contains("1 ambiguous paths"),
        "{result:?}"
    );
    let records = fixture.records();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["file"], "core/src/main/java/com/x/Store.java");
    assert_eq!(
        records[0]["test"],
        "java:project::com::x::StoreTest::testLoad"
    );
}
