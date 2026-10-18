#![cfg(all(feature = "go-grammar", feature = "rhai"))]

use std::fs;
use std::path::Path;
use std::process::Command;

use phronesis_mcp::coverage::select::{FileDiff, changed_regions_from_diffs, select};
use phronesis_mcp::graph::sync;
use phronesis_mcp::language_packs::{TestCommand, changed_pack_symbols, load, render_test_command};

fn write(root: &Path, rel: &str, text: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn git(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {:?}: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn changed_go_body_selects_direct_and_transitive_tests_only_when_graph_is_fresh() {
    let repo = tempfile::tempdir().unwrap();
    let root = repo.path();
    git(root, &["init"]);
    git(root, &["config", "user.email", "test@example.invalid"]);
    git(root, &["config", "user.name", "Test"]);
    write(root, "go.mod", "module example.com/root\n\ngo 1.23\n");
    write(
        root,
        "nested/go.mod",
        "module example.com/nested\n\ngo 1.23\n",
    );
    let old = "package calc\nfunc Work() int { return 1 }\nfunc Helper() int { return Work() }\n";
    let new = "package calc\nfunc Work() int { return 2 }\nfunc Helper() int { return Work() }\n";
    write(root, "nested/calc/work.go", old);
    write(
        root,
        "nested/calc/work_test.go",
        "package calc\nimport \"testing\"\nfunc TestDirect(t *testing.T) { _ = Work() }\nfunc TestViaHelper(t *testing.T) { _ = Helper() }\n",
    );
    write(
        root,
        "other/unrelated.go",
        "package other\nfunc Work() int { return 0 }\n",
    );
    write(
        root,
        "other/unrelated_test.go",
        "package other\nimport \"testing\"\nfunc TestUnrelated(t *testing.T) { _ = Work() }\n",
    );
    git(root, &["add", "."]);
    git(root, &["commit", "-m", "baseline"]);
    sync::rebuild(root).unwrap();
    write(root, "nested/calc/work.go", new);
    let stale = select(root, None).unwrap();
    assert!(!stale.static_reach_available);
    assert!(
        !stale
            .tests
            .iter()
            .any(|test| test.test.contains("TestDirect"))
    );
    sync::rebuild(root).unwrap();
    let selected = select(root, None).unwrap();
    assert!(
        selected.static_reach_available,
        "{:?}",
        selected.static_note
    );
    assert_eq!(
        selected.dynamic_coverage_unsupported,
        vec!["nested/calc/work.go"]
    );
    let tests: Vec<_> = selected
        .tests
        .iter()
        .map(|test| test.test.as_str())
        .collect();
    assert_eq!(tests.len(), 2, "{selected:?}");
    assert!(tests.iter().any(|test| test.contains("TestDirect")));
    assert!(tests.iter().any(|test| test.contains("TestViaHelper")));
    assert!(!tests.iter().any(|test| test.contains("TestUnrelated")));
    for test in &selected.tests {
        assert_eq!(test.evidence, "static_reach");
        let command = test.command.as_ref().expect("Go test command");
        assert!(command.contains("'-run'"));
        assert!(command.contains("^Test"));
    }

    // A deleted declaration is a changed symbol, but is absent from the
    // current graph and cannot select an invented current target.
    let diff = FileDiff {
        path: "nested/calc/work.go".into(),
        old: Some(new.into()),
        new: "package calc\nfunc Helper() int { return 0 }\n".into(),
    };
    let registry = load(root).unwrap();
    let changes = changed_pack_symbols(&registry, root, &[diff]).unwrap();
    assert!(
        changes
            .iter()
            .any(|change| change.symbol.ends_with("::Work"))
    );
    let _ = changed_regions_from_diffs(&[]);
}

#[test]
fn command_rendering_preserves_unicode_quotes_and_shell_metacharacters_as_argv() {
    let repo = tempfile::tempdir().unwrap();
    // Canonicalize so the shell's `$PWD` (which resolves macOS's /var -> /private/var
    // symlink) compares equal to the path we expect.
    let root_buf = repo.path().canonicalize().unwrap();
    let root = root_buf.as_path();
    let directory = "nested/雪 ' ; touch BAD";
    fs::create_dir_all(root.join(directory)).unwrap();
    let script = root.join("fake'go");
    fs::write(&script, "#!/bin/sh\nprintf '%s\\n' \"$PWD\" \"$@\"\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let command = TestCommand {
        program: script.to_string_lossy().into_owned(),
        args: vec![
            "test".into(),
            "./雪 ' ; touch BAD".into(),
            "-run".into(),
            "^TestO'Brien$".into(),
        ],
        cwd: directory.into(),
    };
    let rendered = render_test_command(&command).unwrap();
    // Invoke through sh so this proves quoting behavior, rather than merely
    // comparing a string.
    let output = Command::new("sh")
        .arg("-c")
        .arg(&rendered)
        .current_dir(root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let lines: Vec<_> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect();
    assert_eq!(lines[0], root.join(directory).to_string_lossy());
    assert_eq!(&lines[1..], &command.args);
    assert!(!root.join("BAD").exists());
}
