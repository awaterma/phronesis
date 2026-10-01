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
    let parts: Vec<&str> = rest.split("::").collect();
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
    s.push_str("\nmkdir -p \"$OUT\"\nn=0\n");
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
        s.push_str(&format!("n=$((n+1))\npython3 -m coverage run --source=. --data-file=\"$OUT/{n}.cov\" -m pytest -q -- {}\npython3 -m coverage lcov --data-file=\"$OUT/{n}.cov\" -o \"$OUT/{stem}.lcov\"\nprintf '%s\\n' {} {} | cat - \"$OUT/{stem}.lcov\" > \"$OUT/{n}.tmp\" && mv \"$OUT/{n}.tmp\" \"$OUT/{stem}.lcov\"\nrm -f \"$OUT/{n}.cov\"\n",quote(node),quote(&format!("TN:{id}")),quote(&format!("# node: {node}"))));
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
    }
}
