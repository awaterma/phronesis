//! Task 1 of the language-pack plan: manifest validation, bounded asset
//! reads, and the canonical content digest.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use phronesis_mcp::language_packs::{
    PackAssets, PackError, PackManifest, check_ownership, digest_assets, read_assets,
    validate_manifest,
};
use serde_json::{Value, json};

const SECRET: &str = "OUTSIDE-SECRET-BYTES";

fn manifest() -> Value {
    json!({
        "schema_version": 1,
        "adapter_api": 1,
        "id": "demo_pack",
        "version": "0.1.0",
        "language": "demo",
        "extensions": ["demo"],
        "grammar": "python",
        "capabilities": ["structural_facts", "toolchains"],
        "queries": [{"id": "decls", "path": "queries/decls.scm", "emit": [
            {"kind": "declaration", "name": "${name}", "span_capture": "def"}
        ]}],
        "adapter": "adapter.rhai",
        "rules": "rules.json",
        "toolchains": "toolchains.json",
        "context_files": ["demo.mod"],
        "fixtures": [{"path": "fixtures/a.demo", "expected": "fixtures/a.json"}]
    })
}

fn write(dir: &Path, rel: &str, bytes: &[u8]) {
    let path = dir.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

/// Writes every resource `manifest` declares, then the manifest itself.
fn write_pack(dir: &Path, manifest: &Value) {
    let mut paths: Vec<String> = Vec::new();
    for q in manifest["queries"].as_array().into_iter().flatten() {
        paths.push(q["path"].as_str().unwrap().to_string());
    }
    for key in ["adapter", "rules", "toolchains"] {
        if let Some(p) = manifest[key].as_str() {
            paths.push(p.to_string());
        }
    }
    for f in manifest["fixtures"].as_array().into_iter().flatten() {
        paths.push(f["path"].as_str().unwrap().to_string());
        paths.push(f["expected"].as_str().unwrap().to_string());
    }
    for p in paths {
        let writable = !p.is_empty() && !p.starts_with('/') && !p.ends_with('/');
        if writable && !p.contains("..") && !dir.join(&p).exists() {
            write(dir, &p, format!("contents of {p}\n").as_bytes());
        }
    }
    write(dir, "pack.json", manifest.to_string().as_bytes());
}

fn read_err(manifest: &Value) -> PackError {
    let tmp = tempfile::tempdir().unwrap();
    write_pack(tmp.path(), manifest);
    read_assets(tmp.path()).expect_err("pack should be rejected")
}

fn assert_mentions(err: &PackError, needles: &[&str]) {
    let text = err.to_string();
    for needle in needles {
        assert!(text.contains(needle), "{text:?} should mention {needle:?}");
    }
    assert!(!text.contains(SECRET), "{text:?} leaked outside bytes");
}

#[test]
fn valid_pack_reads_declared_assets() {
    let tmp = tempfile::tempdir().unwrap();
    write_pack(tmp.path(), &manifest());
    let assets = read_assets(tmp.path()).unwrap();
    assert_eq!(assets.manifest.id, "demo_pack");
    let names: Vec<&str> = assets.files.keys().map(String::as_str).collect();
    assert_eq!(
        names,
        [
            "adapter.rhai",
            "fixtures/a.demo",
            "fixtures/a.json",
            "pack.json",
            "queries/decls.scm",
            "rules.json",
            "toolchains.json",
        ]
    );
    validate_manifest(&assets.manifest).unwrap();
}

#[test]
fn unknown_fields_and_versions_fail() {
    let mut m = manifest();
    m["extra"] = json!(true);
    let err = read_err(&m);
    assert_mentions(&err, &["extra", "pack.json"]);
    assert_eq!(err.stage(), "manifest");

    let mut m = manifest();
    m["queries"][0]["bogus"] = json!(1);
    assert_mentions(&read_err(&m), &["bogus"]);

    let mut m = manifest();
    m["fixtures"][0]["command"] = json!("rm -rf /");
    assert_mentions(&read_err(&m), &["command"]);

    for (field, value) in [
        ("schema_version", json!(2)),
        ("adapter_api", json!(0)),
        ("version", json!("1.0")),
        ("version", json!("1.0.x")),
        ("id", json!("Demo")),
        ("id", json!("9demo")),
        ("language", json!("de-mo")),
    ] {
        let mut m = manifest();
        m[field] = value;
        let err = read_err(&m);
        assert_mentions(&err, &[field]);
        assert_eq!(err.stage(), "manifest", "{err}");
    }

    let mut m = manifest();
    m["schema_version"] = json!(2);
    assert_eq!(read_err(&m).pack(), Some("demo_pack"));

    let mut m = manifest();
    m["capabilities"] = json!(["coverage"]);
    assert_mentions(&read_err(&m), &["coverage"]);

    let mut m = manifest();
    m["grammar"] = json!("cobol");
    assert_mentions(&read_err(&m), &["cobol", "go", "rust", "tsx"]);

    // Contradictory declarations.
    let mut m = manifest();
    m["capabilities"] = json!(["structural_facts"]);
    assert_mentions(&read_err(&m), &["toolchains"]);
    let mut m = manifest();
    m["toolchains"] = Value::Null;
    assert_mentions(&read_err(&m), &["toolchains"]);
    let mut m = manifest();
    m["queries"] = json!([]);
    assert_mentions(&read_err(&m), &["queries"]);

    let mut m = manifest();
    m["queries"][0]["emit"][0]["kind"] = json!("edge");
    assert_mentions(&read_err(&m), &["edge"]);

    // The same checks are available without touching disk.
    let mut parsed: PackManifest = serde_json::from_value(manifest()).unwrap();
    parsed.adapter_api = 2;
    let err = validate_manifest(&parsed).unwrap_err();
    assert_mentions(&err, &["adapter_api", "demo_pack"]);
}

#[test]
fn resources_cannot_escape_pack() {
    let outer = tempfile::tempdir().unwrap();
    write(outer.path(), "outside.scm", SECRET.as_bytes());
    write(outer.path(), "outside/decls.scm", SECRET.as_bytes());
    let pack = outer.path().join("pack");
    fs::create_dir(&pack).unwrap();

    let bad_paths = [
        "../outside.scm",
        "/etc/passwd",
        "queries/../../outside.scm",
        "queries/./decls.scm",
        "queries//decls.scm",
        "queries\\decls.scm",
        "",
        "queries/",
        "C:/outside.scm",
        "C:outside.scm",
        "queries/C:/outside.scm",
        "queries/decls.scm:stream",
    ];
    for bad in bad_paths {
        let mut m = manifest();
        m["queries"][0]["path"] = json!(bad);
        let _ = fs::remove_dir_all(&pack);
        fs::create_dir(&pack).unwrap();
        write_pack(&pack, &m);
        let err = read_assets(&pack).expect_err(bad);
        assert_mentions(&err, &["demo_pack"]);
        assert_eq!(err.path(), Some(bad), "{err}");
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;

        // Directory component is a symlink to outside the pack.
        let _ = fs::remove_dir_all(&pack);
        fs::create_dir(&pack).unwrap();
        symlink(outer.path().join("outside"), pack.join("queries")).unwrap();
        write_pack(&pack, &manifest());
        let err = read_assets(&pack).unwrap_err();
        assert_mentions(&err, &["queries/decls.scm", "symlink"]);

        // Final component is a symlink.
        let _ = fs::remove_dir_all(&pack);
        fs::create_dir_all(pack.join("queries")).unwrap();
        symlink(
            outer.path().join("outside.scm"),
            pack.join("queries/decls.scm"),
        )
        .unwrap();
        write_pack(&pack, &manifest());
        assert_mentions(&read_assets(&pack).unwrap_err(), &["symlink"]);

        // The manifest itself is a symlink.
        let _ = fs::remove_dir_all(&pack);
        fs::create_dir(&pack).unwrap();
        write_pack(&pack, &manifest());
        fs::rename(pack.join("pack.json"), outer.path().join("real.json")).unwrap();
        symlink(outer.path().join("real.json"), pack.join("pack.json")).unwrap();
        assert_mentions(&read_assets(&pack).unwrap_err(), &["pack.json", "symlink"]);

        // The pack root is a symlink.
        let _ = fs::remove_dir_all(&pack);
        let real = outer.path().join("real-pack");
        fs::create_dir(&real).unwrap();
        write_pack(&real, &manifest());
        symlink(&real, &pack).unwrap();
        assert_mentions(&read_assets(&pack).unwrap_err(), &["symlink"]);
        fs::remove_file(&pack).unwrap();

        // A FIFO is not a regular file and must not block the reader.
        fs::create_dir_all(pack.join("queries")).unwrap();
        let fifo = std::ffi::CString::new(
            pack.join("queries/decls.scm")
                .as_os_str()
                .as_encoded_bytes(),
        )
        .unwrap();
        // SAFETY: `fifo` is a valid NUL-terminated path for the call.
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        write_pack(&pack, &manifest());
        assert_mentions(&read_assets(&pack).unwrap_err(), &["queries/decls.scm"]);
        fs::remove_dir_all(&pack).unwrap();
    }

    // A declared directory is not a regular file.
    fs::create_dir_all(pack.join("queries/decls.scm")).unwrap();
    write_pack(&pack, &manifest());
    assert_mentions(&read_assets(&pack).unwrap_err(), &["queries/decls.scm"]);

    // Context files are basenames.
    for bad in ["sub/demo.mod", "..", ".", "", "a b/c"] {
        let mut m = manifest();
        m["context_files"] = json!([bad]);
        assert_mentions(&read_err(&m), &["context_files"]);
    }
    let mut m = manifest();
    m["context_files"] = json!(["a", "b", "c", "d", "e"]);
    assert_mentions(&read_err(&m), &["context_files"]);
}

#[test]
fn digest_covers_every_declared_asset() {
    let tmp = tempfile::tempdir().unwrap();
    write_pack(tmp.path(), &manifest());
    let base = digest_assets(&read_assets(tmp.path()).unwrap());
    assert!(base.starts_with("sha256:"), "{base}");
    assert_eq!(base.len(), "sha256:".len() + 64);
    assert_eq!(base, digest_assets(&read_assets(tmp.path()).unwrap()));

    // Undeclared files are neither read nor hashed.
    write(tmp.path(), "README.md", b"notes");
    let assets = read_assets(tmp.path()).unwrap();
    assert!(!assets.files.contains_key("README.md"));
    assert_eq!(digest_assets(&assets), base);

    for rel in [
        "pack.json",
        "queries/decls.scm",
        "adapter.rhai",
        "rules.json",
        "toolchains.json",
        "fixtures/a.demo",
        "fixtures/a.json",
    ] {
        let path = tmp.path().join(rel);
        let original = fs::read(&path).unwrap();
        let mut changed = original.clone();
        changed.push(b' ');
        fs::write(&path, &changed).unwrap();
        let digest = digest_assets(&read_assets(tmp.path()).unwrap());
        assert_ne!(digest, base, "changing {rel} must change the digest");
        fs::write(&path, &original).unwrap();
    }
    assert_eq!(digest_assets(&read_assets(tmp.path()).unwrap()), base);

    // Length prefixes keep name/content boundaries unambiguous.
    let assets = read_assets(tmp.path()).unwrap();
    let with = |files: &[(&str, &str)]| PackAssets {
        manifest: assets.manifest.clone(),
        files: files
            .iter()
            .map(|(k, v)| (k.to_string(), v.as_bytes().to_vec()))
            .collect::<BTreeMap<_, _>>(),
    };
    assert_ne!(
        digest_assets(&with(&[("ab", "c")])),
        digest_assets(&with(&[("a", "bc")]))
    );
    assert_ne!(
        digest_assets(&with(&[("a", "b"), ("c", "d")])),
        digest_assets(&with(&[("a", "bcd")]))
    );
}

#[test]
fn duplicate_paths_and_ownership_are_rejected() {
    let mut m = manifest();
    m["queries"] = json!([
        {"id": "a", "path": "queries/decls.scm", "emit": []},
        {"id": "b", "path": "queries/decls.scm", "emit": []}
    ]);
    assert_mentions(&read_err(&m), &["queries/decls.scm", "duplicate"]);

    let mut m = manifest();
    m["fixtures"][0]["path"] = json!("queries/decls.scm");
    assert_mentions(&read_err(&m), &["queries/decls.scm", "duplicate"]);

    let mut m = manifest();
    m["fixtures"][0]["path"] = json!("Queries/Decls.scm");
    assert_mentions(&read_err(&m), &["duplicate"]);

    let mut m = manifest();
    m["queries"] = json!([
        {"id": "a", "path": "queries/a.scm", "emit": []},
        {"id": "a", "path": "queries/b.scm", "emit": []}
    ]);
    assert_mentions(&read_err(&m), &["queries", "duplicate"]);

    for reserved in ["pack.json", "installed.json"] {
        let mut m = manifest();
        m["rules"] = json!(reserved);
        assert_mentions(&read_err(&m), &[reserved]);
    }

    let mut m = manifest();
    m["extensions"] = json!(["demo", "demo"]);
    assert_mentions(&read_err(&m), &["extensions", "duplicate"]);
    for bad in [".demo", "de/mo", "*.demo", "de mo", "", "de?mo", "de\\mo"] {
        let mut m = manifest();
        m["extensions"] = json!([bad]);
        assert_mentions(&read_err(&m), &["extensions"]);
    }
    let mut m = manifest();
    m["extensions"] = json!([]);
    assert_mentions(&read_err(&m), &["extensions"]);

    let parse = |id: &str, exts: &[&str]| -> PackManifest {
        let mut m = manifest();
        m["id"] = json!(id);
        m["extensions"] = json!(exts);
        serde_json::from_value(m).unwrap()
    };
    let a = parse("alpha", &["demo"]);
    let b = parse("beta", &["demo"]);
    let err = check_ownership([&a, &b], &[]).unwrap_err();
    assert_mentions(&err, &["demo", "alpha", "beta"]);
    assert_eq!(err.stage(), "ownership");

    let a2 = parse("alpha", &["other"]);
    assert_mentions(
        &check_ownership([&a, &a2], &[]).unwrap_err(),
        &["alpha", "duplicate"],
    );

    let rs = parse("gamma", &["rs"]);
    assert_mentions(
        &check_ownership([&rs], &[("rs", "rust")]).unwrap_err(),
        &["rs", "rust", "gamma"],
    );

    // Ownership is exact and case-sensitive.
    let upper = parse("beta", &["Demo"]);
    check_ownership([&a, &upper], &[("rs", "rust")]).unwrap();
}

#[test]
fn asset_caps_are_enforced() {
    let kib = 1024usize;

    // Manifest over 64 KiB (padding is valid JSON whitespace).
    let tmp = tempfile::tempdir().unwrap();
    write_pack(tmp.path(), &manifest());
    let mut text = manifest().to_string();
    text.push_str(&" ".repeat(64 * kib));
    write(tmp.path(), "pack.json", text.as_bytes());
    let err = read_assets(tmp.path()).unwrap_err();
    assert_mentions(&err, &["pack.json"]);
    assert_eq!(err.stage(), "limit");

    // Query and script over 64 KiB; fixture over 1 MiB.
    for (rel, size) in [
        ("queries/decls.scm", 64 * kib + 1),
        ("adapter.rhai", 64 * kib + 1),
        ("fixtures/a.demo", 1024 * kib + 1),
    ] {
        let tmp = tempfile::tempdir().unwrap();
        write_pack(tmp.path(), &manifest());
        write(tmp.path(), rel, &vec![b'x'; size]);
        let err = read_assets(tmp.path()).unwrap_err();
        assert_mentions(&err, &[rel, "demo_pack"]);
        assert_eq!(err.stage(), "limit");
    }
    // Exactly at the cap is accepted.
    let tmp = tempfile::tempdir().unwrap();
    write_pack(tmp.path(), &manifest());
    write(tmp.path(), "queries/decls.scm", &vec![b'x'; 64 * kib]);
    read_assets(tmp.path()).unwrap();

    // Total bytes over 8 MiB with every file individually within its cap.
    let tmp = tempfile::tempdir().unwrap();
    let mut m = manifest();
    m["fixtures"] = json!(
        (0..5)
            .map(|i| json!({"path": format!("fixtures/{i}.demo"), "expected": format!("fixtures/{i}.json")}))
            .collect::<Vec<_>>()
    );
    for i in 0..5 {
        write(
            tmp.path(),
            &format!("fixtures/{i}.demo"),
            &vec![b'x'; 1024 * kib],
        );
        write(
            tmp.path(),
            &format!("fixtures/{i}.json"),
            &vec![b'x'; 1024 * kib],
        );
    }
    write_pack(tmp.path(), &m);
    let err = read_assets(tmp.path()).unwrap_err();
    assert_mentions(&err, &["8388608"]);
    assert_eq!(err.stage(), "limit");

    // More than 32 queries.
    let mut m = manifest();
    m["queries"] = json!(
        (0..33)
            .map(|i| json!({"id": format!("q{i}"), "path": format!("queries/{i}.scm"), "emit": []}))
            .collect::<Vec<_>>()
    );
    assert_mentions(&read_err(&m), &["queries", "32"]);

    // More than 128 assets (manifest included).
    let mut m = manifest();
    m["fixtures"] = json!(
        (0..63)
            .map(|i| json!({"path": format!("fixtures/{i}.demo"), "expected": format!("fixtures/{i}.json")}))
            .collect::<Vec<_>>()
    );
    assert_mentions(&read_err(&m), &["128"]);
}
