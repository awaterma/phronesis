//! pytest collection helpers for producing one isolated LCOV file per test.
use std::path::Path;

pub fn parse_collect_only(output: &str) -> Vec<String> {
    output
        .lines()
        .map(str::trim)
        .filter(|line| {
            line.contains(".py::") && !line.starts_with("=") && !line.starts_with("WARNING")
        })
        .map(str::to_string)
        .collect()
}

pub fn graph_test_id(namespace: &str, node_id: &str) -> Option<String> {
    let (file, rest) = node_id.split_once("::")?;
    let parts: Vec<&str> = rest.split('[').next()?.split("::").collect();
    let last = parts.last()?.split('[').next()?;
    if !last.starts_with("test_") {
        return None;
    }
    let mut graph_parts = parts;
    let final_part = graph_parts.last_mut()?;
    *final_part = last;
    Some(crate::graph::python::qualified_test_id(
        namespace,
        file,
        &graph_parts,
    ))
}

pub fn file_stem_for(graph_id: &str) -> String {
    graph_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

pub fn collection_script(entries: &[(String, String)], out_dir: &Path) -> String {
    let mut s = String::from(
        "#!/bin/sh\nset -eu\npython3 -m coverage --version\npython3 -m pytest --version\nOUT=",
    );
    s.push_str(&quote(&out_dir.to_string_lossy()));
    s.push_str("\nmkdir -p \"$OUT\"\nfind \"$OUT\" -maxdepth 1 -type f \\( -name '*.lcov' -o -name '*.info' -o -name '*.cov' -o -name 'manifest.json' \\) -delete\nn=0\n");
    let mut stems = std::collections::BTreeMap::<String, usize>::new();
    for (index, (node, id)) in entries.iter().enumerate() {
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
        s.push_str(&format!("n=$((n+1))\nrm -f \"$OUT/{n}.junit.xml\"\npython3 -m coverage run --source=. --data-file=\"$OUT/{n}.cov\" -m pytest -q --junitxml=\"$OUT/{n}.junit.xml\" -o junit_family=xunit2 -- {}\n", quote(node)));
        s.push_str(&format!(
            "python3 - \"$OUT/{n}.junit.xml\" {} <<'CHECK'\n",
            quote(node)
        ));
        s.push_str(r#"import sys,xml.etree.ElementTree as ET
cases=list(ET.parse(sys.argv[1]).getroot().iter('testcase'))
if len(cases)!=1 or any(c.find(tag) is not None for c in cases for tag in ('failure','error','skipped')):
 raise SystemExit('expected exactly one successful pytest test')
selector,sep,parameter=sys.argv[2].partition('[')
parts=selector.split('::')
name=parts[-1]+(sep+parameter if sep else '')
classname=parts[0][:-3].replace('/','.')
if len(parts)>2: classname+='.'+'.'.join(parts[1:-1])
if cases[0].get('classname')!=classname or cases[0].get('name')!=name:
 raise SystemExit('pytest executed a different test than requested')
CHECK
"#);
        s.push_str(&format!("python3 -m coverage lcov --data-file=\"$OUT/{n}.cov\" -o \"$OUT/{stem}.lcov\"\nprintf '%s\\n' {} {} | cat - \"$OUT/{stem}.lcov\" > \"$OUT/{n}.tmp\" && mv \"$OUT/{n}.tmp\" \"$OUT/{stem}.lcov\"\nrm -f \"$OUT/{n}.cov\"\n", quote(&format!("TN:{id}")), quote(&format!("# node: {node}"))));
    }
    s.push_str("python3 - \"$OUT\" <<'PY'\nimport hashlib,json,os,subprocess,sys\nout=sys.argv[1]; root=subprocess.check_output(['git','rev-parse','--show-toplevel'],text=True).strip(); files={}\nfor name in sorted(os.listdir(out)):\n if name.endswith(('.lcov','.info')):\n  for line in open(os.path.join(out,name)):\n   if line.startswith('SF:'):\n    p=os.path.realpath(line[3:].strip()); rel=os.path.relpath(p,root)\n    if rel.startswith('..'+os.sep): raise SystemExit('source outside git root: '+p)\n    files[rel]=hashlib.sha256(open(p,'rb').read()).hexdigest()\nrev=subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip()\njson.dump({'revision':rev,'files':files},open(os.path.join(out,'manifest.json'),'w'),sort_keys=True)\nPY\n");
    s.push_str(&format!(
        "echo {}\n",
        quote(&format!(
            "phr-mcp coverage import --format lcov-dir --tool coverage.py {}",
            out_dir.display()
        ))
    ));
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_only_pytest_node_ids() {
        let ids = parse_collect_only(include_str!("../../tests/fixtures/pytest/collect-only.txt"));
        assert_eq!(ids.len(), 3);
        assert!(ids[1].contains("::TestSave::test_it"));
        assert!(ids[2].ends_with(']'));
    }
    #[test]
    fn graph_id_and_safe_stem() {
        assert_eq!(
            graph_test_id("pkg", "tests/test_store.py::test_load").as_deref(),
            Some("python:pkg::tests::test_store::test_load")
        );
        assert_eq!(
            graph_test_id("pkg", "tests/test_store.py::TestSave::test_it[a-b]").as_deref(),
            Some("python:pkg::tests::test_store::TestSave::test_it")
        );
        assert_eq!(
            file_stem_for("python:pkg::tests::test_store::test_load"),
            "python_pkg__tests__test_store__test_load"
        );
    }
    #[test]
    fn script_quotes_and_manifests() {
        let s = collection_script(
            &[(
                "tests/test_store.py::test_it['x y']".into(),
                "python:pkg::tests::test_store::test_it".into(),
            )],
            Path::new("/tmp/cov"),
        );
        assert!(s.contains("'tests/test_store.py::test_it['\\''x y'\\'']'"));
        assert!(s.contains("manifest.json") && s.contains("rev-parse"));
    }

    #[test]
    #[ignore = "requires pytest and coverage.py; run explicitly in Python collector CI"]
    fn real_pytest_collector_rejects_skips_and_accepts_exact_parameter_nodes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        std::fs::create_dir(root.join("tests")).expect("tests");
        std::fs::write(root.join("store.py"), "def load():\n    return 7\nload()\n")
            .expect("source");
        std::fs::write(root.join("tests/test_store.py"), "import pytest\nfrom store import load\n@pytest.mark.skip(reason='skip fixture')\ndef test_skipped():\n    assert load() == 7\n@pytest.mark.parametrize('value',[7,8,9],ids=['a::b','first case', \"quote'case\"])\ndef test_load(value):\n    assert load() == 7\n").expect("tests");
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
        for (node, success) in [
            ("tests/test_store.py::test_skipped", false),
            ("tests/test_store.py::test_load[a::b]", true),
            ("tests/test_store.py::test_load[first case]", true),
            ("tests/test_store.py::test_load[quote'case]", true),
            ("tests/test_store.py::test_skipped", false),
        ] {
            let id = graph_test_id("example", node).expect("id");
            std::fs::write(&script, collection_script(&[(node.into(), id)], &out)).expect("script");
            let result = std::process::Command::new("sh")
                .arg(&script)
                .current_dir(root)
                .env("PYTHONPATH", root)
                .output()
                .expect("collect");
            assert_eq!(
                result.status.success(),
                success,
                "{node}: stdout={} stderr={}",
                String::from_utf8_lossy(&result.stdout),
                String::from_utf8_lossy(&result.stderr)
            );
            assert_eq!(out.join("manifest.json").exists(), success);
            if !success {
                assert!(
                    String::from_utf8_lossy(&result.stderr)
                        .contains("expected exactly one successful pytest test")
                );
                assert!(!std::fs::read_dir(&out).expect("out").any(|entry| {
                    entry
                        .expect("entry")
                        .path()
                        .extension()
                        .is_some_and(|ext| ext == "lcov")
                }));
            }
        }
    }

    #[test]
    #[ignore = "requires pytest and coverage.py; run explicitly in Python collector CI"]
    fn real_pytest_collection_excludes_external_imports_and_keeps_production_hits() {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().join("repo");
        let external = dir.path().join("external");
        std::fs::create_dir_all(root.join("pkg")).expect("package");
        std::fs::create_dir_all(root.join("tests")).expect("tests");
        std::fs::create_dir(&external).expect("external");
        std::fs::write(
            external.join("outside.py"),
            "def external_value():\n    return 7\n",
        )
        .expect("external source");
        std::fs::write(root.join("pkg/__init__.py"), "").expect("init");
        std::fs::write(root.join("pkg/store.py"), "def load():\n    return 7\n")
            .expect("production");
        std::fs::write(root.join("tests/test_store.py"), "from pkg.store import load\nfrom outside import external_value\ndef test_load():\n    assert load() == external_value()\n").expect("test");
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
                std::process::Command::new("git")
                    .args(args)
                    .current_dir(&root)
                    .status()
                    .expect("git")
                    .success()
            );
        }
        let out = root.join("coverage");
        std::fs::create_dir(&out).expect("output");
        std::fs::write(out.join("stale.lcov"), "SF:deleted.py\nDA:1,1\n").expect("stale report");
        std::fs::write(out.join("manifest.json"), "stale").expect("stale manifest");
        std::fs::write(out.join("stale.cov"), "stale").expect("stale data");
        let script = root.join("collect.sh");
        std::fs::write(
            &script,
            collection_script(
                &[(
                    "tests/test_store.py::test_load".into(),
                    "python:pkg::tests::test_store::test_load".into(),
                )],
                &out,
            ),
        )
        .expect("script");
        let result = std::process::Command::new("sh")
            .arg(&script)
            .current_dir(&root)
            .env("PYTHONPATH", &external)
            .output()
            .expect("collector");
        assert!(
            result.status.success(),
            "stdout={} stderr={}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        let report =
            std::fs::read_to_string(out.join("python_pkg__tests__test_store__test_load.lcov"))
                .expect("lcov");
        assert!(report.contains("SF:pkg/store.py"), "{report}");
        assert!(report.contains("DA:2,1"), "{report}");
        assert!(!out.join("stale.lcov").exists());
        assert!(!out.join("stale.cov").exists());
        assert!(!report.contains("outside.py"));
        assert!(!report.contains("_pytest"));
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(out.join("manifest.json")).expect("manifest"))
                .expect("JSON");
        assert!(manifest["files"].get("pkg/store.py").is_some());
        assert!(
            manifest["files"]
                .as_object()
                .expect("files")
                .keys()
                .all(|name| !name.contains("outside") && !name.contains("_pytest"))
        );
        std::fs::write(
            root.join("tests/test_store.py"),
            "def test_load():\n    assert False\n",
        )
        .expect("failing test");
        let failed = std::process::Command::new("sh")
            .arg(&script)
            .current_dir(&root)
            .env("PYTHONPATH", &external)
            .output()
            .expect("failed rerun");
        assert!(!failed.status.success());
        assert!(!out.join("manifest.json").exists());
        assert!(
            !out.join("python_pkg__tests__test_store__test_load.lcov")
                .exists()
        );
    }
}
