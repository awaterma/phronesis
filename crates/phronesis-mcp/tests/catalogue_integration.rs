use std::fs;
use std::process::Command;

fn run(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_phr-mcp"))
        .arg("catalogue")
        .args(args)
        .output()
        .expect("failed to spawn phr-mcp")
}

#[test]
fn regenerates_between_markers() {
    let dir = tempfile::tempdir().unwrap();
    let page = dir.path().join("catalogue.html");
    fs::write(
        &page,
        "<header>k</header>\n<!-- BEGIN GENERATED RULES -->\nSTALE\n<!-- END GENERATED RULES -->\n<footer>k</footer>",
    )
    .unwrap();

    let out = run(&["--out", page.to_str().unwrap()]);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let html = fs::read_to_string(&page).unwrap();
    assert!(!html.contains("STALE"));
    assert!(html.contains("<article class=\"rule\""));
    assert!(html.contains("<header>k</header>"));
    assert!(html.contains("<footer>k</footer>"));
}

#[test]
fn missing_markers_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let page = dir.path().join("catalogue.html");
    fs::write(&page, "<p>no markers</p>").unwrap();

    let out = run(&["--out", page.to_str().unwrap()]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("marker"));
}

#[test]
fn missing_file_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let page = dir.path().join("does-not-exist.html");

    let out = run(&["--out", page.to_str().unwrap()]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("error"));
}
