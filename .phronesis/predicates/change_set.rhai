// Classify an apply_patch operation as a whole. Batch events have `files`
// populated and `file_path` empty; per-file events have the inverse shape.
let production_paths = [];
let test_paths = [];

for path in event.files {
    let is_rust = path.ends_with(".rs");
    let is_production = is_rust
        && (path.starts_with("src/") || path.contains("/src/"));
    let is_test = is_rust
        && (path.starts_with("tests/")
            || path.contains("/tests/")
            || path.ends_with("_test.rs")
            || path.ends_with("_tests.rs"));

    if is_production {
        production_paths.push(path);
        emit_fact("change_set_production_rust", [path]);
    }
    if is_test {
        test_paths.push(path);
        emit_fact("change_set_test", [path]);
    }
}

if production_paths.len > 0 {
    emit_fact("change_set_has_production_rust", []);
}
if test_paths.len > 0 {
    emit_fact("change_set_has_test", []);
}
if production_paths.len > 0 && test_paths.len == 0 {
    emit_fact("change_set_production_without_test", []);
}
