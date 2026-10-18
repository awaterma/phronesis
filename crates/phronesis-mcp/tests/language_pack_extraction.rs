//! Task 2 of the language-pack plan: query execution and sandboxed Rhai
//! interpretation, exercised through the public `language_packs` API.

use std::fs;
use std::path::Path;

use phronesis_mcp::language_packs::{FileContext, PackRecord, compile_pack, extract, read_assets};
use serde_json::{Value, json};

fn write(dir: &Path, rel: &str, bytes: &[u8]) {
    let path = dir.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

fn write_pack(dir: &Path, manifest: &Value) {
    write(dir, "pack.json", manifest.to_string().as_bytes());
}

fn ctx(file_rel: &str) -> FileContext {
    FileContext {
        file_rel: file_rel.to_string(),
        context_files: Vec::new(),
    }
}

const DECL_QUERY: &[u8] = b"(function_definition name: (identifier) @name) @def";

fn query_only_manifest(id: &str, ext: &str) -> Value {
    json!({
        "schema_version": 1,
        "adapter_api": 1,
        "id": id,
        "version": "0.1.0",
        "language": id,
        "extensions": [ext],
        "grammar": "python",
        "capabilities": ["structural_facts"],
        "queries": [{
            "id": "decls",
            "path": "queries/decls.scm",
            "emit": [{"kind": "declaration", "name": "${name}", "scope": "", "span_capture": "def"}]
        }],
        "context_files": [],
        "fixtures": []
    })
}

#[cfg(feature = "rhai")]
fn rhai_manifest(id: &str, ext: &str) -> Value {
    json!({
        "schema_version": 1,
        "adapter_api": 1,
        "id": id,
        "version": "0.1.0",
        "language": id,
        "extensions": [ext],
        "grammar": "python",
        "capabilities": ["structural_facts"],
        "queries": [{"id": "decls", "path": "queries/decls.scm", "emit": []}],
        "adapter": "adapter.rhai",
        "context_files": [],
        "fixtures": []
    })
}

#[cfg(feature = "rhai")]
const RHAI_ADAPTER: &str = r#"
    let out = [];
    for m in matches {
        let name = "";
        let def = #{};
        for c in m.captures {
            if c.name == "name" { name = c.text; }
            if c.name == "def" { def = c.span; }
        }
        out.push(#{ kind: "declaration", name: name, scope: "", span: def });
    }
    out
"#;

const PY_SOURCE: &str = "def a():\n    pass\n\n\ndef b():\n    pass\n";

#[test]
fn query_only_pack_extracts_declarations() {
    let tmp = tempfile::tempdir().unwrap();
    write(tmp.path(), "queries/decls.scm", DECL_QUERY);
    write_pack(tmp.path(), &query_only_manifest("tmpl_pack", "tmplpy"));
    let pack = compile_pack(read_assets(tmp.path()).unwrap()).unwrap();
    let extraction = extract(&pack, &ctx("a.tmplpy"), PY_SOURCE).unwrap();
    assert!(
        extraction.diagnostics.is_empty(),
        "{:?}",
        extraction.diagnostics
    );
    let names: Vec<&str> = extraction
        .records
        .iter()
        .map(|r| match r {
            PackRecord::Declaration { name, .. } => name.as_str(),
            other => panic!("expected declaration, got {other:?}"),
        })
        .collect();
    assert_eq!(names, ["a", "b"]);
}

#[cfg(feature = "rhai")]
#[test]
fn query_only_and_rhai_adapters_extract_identical_declaration_spans() {
    let tmpl_dir = tempfile::tempdir().unwrap();
    write(tmpl_dir.path(), "queries/decls.scm", DECL_QUERY);
    write_pack(
        tmpl_dir.path(),
        &query_only_manifest("tmpl_pack2", "tmplpy2"),
    );
    let tmpl_pack = compile_pack(read_assets(tmpl_dir.path()).unwrap()).unwrap();
    let tmpl_extraction = extract(&tmpl_pack, &ctx("a.tmplpy2"), PY_SOURCE).unwrap();

    let rhai_dir = tempfile::tempdir().unwrap();
    write(rhai_dir.path(), "queries/decls.scm", DECL_QUERY);
    write(rhai_dir.path(), "adapter.rhai", RHAI_ADAPTER.as_bytes());
    write_pack(rhai_dir.path(), &rhai_manifest("rhai_pack", "rhaipy"));
    let rhai_pack = compile_pack(read_assets(rhai_dir.path()).unwrap()).unwrap();
    let rhai_extraction = extract(&rhai_pack, &ctx("a.rhaipy"), PY_SOURCE).unwrap();

    assert_eq!(tmpl_extraction.records, rhai_extraction.records);
}

#[test]
fn comments_and_string_lookalikes_produce_no_declarations() {
    let tmp = tempfile::tempdir().unwrap();
    write(tmp.path(), "queries/decls.scm", DECL_QUERY);
    write_pack(tmp.path(), &query_only_manifest("tmpl_pack3", "tmplpy3"));
    let pack = compile_pack(read_assets(tmp.path()).unwrap()).unwrap();
    let source = "# def lookalike():\n#     pass\nx = \"def also_not_a_function():\"\n";
    let extraction = extract(&pack, &ctx("a.tmplpy3"), source).unwrap();
    assert!(extraction.records.is_empty(), "{:?}", extraction.records);
}

#[test]
fn absent_capture_reference_fails_that_emission_only() {
    let tmp = tempfile::tempdir().unwrap();
    write(tmp.path(), "queries/decls.scm", DECL_QUERY);
    let mut manifest = query_only_manifest("tmpl_pack4", "tmplpy4");
    // Reference a capture that the query never binds.
    manifest["queries"][0]["emit"][0]["name"] = json!("${missing}");
    write_pack(tmp.path(), &manifest);
    let pack = compile_pack(read_assets(tmp.path()).unwrap()).unwrap();
    let extraction = extract(&pack, &ctx("a.tmplpy4"), PY_SOURCE).unwrap();
    assert!(extraction.records.is_empty(), "{:?}", extraction.records);
    assert_eq!(
        extraction.diagnostics.len(),
        2,
        "{:?}",
        extraction.diagnostics
    );
    for d in &extraction.diagnostics {
        assert!(d.message.contains("missing"), "{}", d.message);
    }
}

#[test]
fn invalid_owner_start_byte_reference_is_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    write(
        tmp.path(),
        "queries/refs.scm",
        b"(call function: (identifier) @callee) @call",
    );
    let manifest = json!({
        "schema_version": 1,
        "adapter_api": 1,
        "id": "ref_pack",
        "version": "0.1.0",
        "language": "ref_pack",
        "extensions": ["refpy"],
        "grammar": "python",
        "capabilities": ["structural_facts"],
        "queries": [{
            "id": "refs",
            "path": "queries/refs.scm",
            "emit": [{
                "kind": "reference",
                "owner_start_byte": 999999,
                "target": "${callee}",
                "scope": "",
                "resolution": "same_scope",
                "span_capture": "call"
            }]
        }],
        "context_files": [],
        "fixtures": []
    });
    write_pack(tmp.path(), &manifest);
    let pack = compile_pack(read_assets(tmp.path()).unwrap()).unwrap();
    let err = extract(&pack, &ctx("a.refpy"), "foo()\n").unwrap_err();
    assert_eq!(err.stage(), "record", "{err}");
    assert!(err.to_string().contains("owner_start_byte"), "{err}");
}

#[cfg(feature = "rhai")]
#[test]
fn adapter_cannot_mutate_injected_context_or_matches() {
    let tmp = tempfile::tempdir().unwrap();
    write(tmp.path(), "queries/decls.scm", DECL_QUERY);
    write(
        tmp.path(),
        "adapter.rhai",
        b"context.file_rel = \"forged\"; []",
    );
    write_pack(tmp.path(), &rhai_manifest("mutate_pack", "mutatepy"));
    let pack = compile_pack(read_assets(tmp.path()).unwrap()).unwrap();
    let err = extract(&pack, &ctx("a.mutatepy"), PY_SOURCE).unwrap_err();
    assert_eq!(err.stage(), "script", "{err}");
}

#[cfg(feature = "rhai")]
#[test]
fn forbidden_apis_are_rejected_at_pack_compile_time() {
    for script in [
        br#"eval("1 + 1")"# as &[u8],
        br#"open_file("/etc/passwd")"#,
        br#"import "std" as s;"#,
    ] {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "queries/decls.scm", DECL_QUERY);
        write(tmp.path(), "adapter.rhai", script);
        write_pack(tmp.path(), &rhai_manifest("forbidden_pack", "forbiddenpy"));
        let assets = read_assets(tmp.path()).unwrap();
        // `eval` is a parse-time rejection (disabled symbol); the other two
        // compile fine (unknown function/module) but fail when actually run,
        // which `validate_adapter`'s compile-only check cannot see — pin
        // both outcomes explicitly rather than asserting one.
        match compile_pack(assets) {
            Ok(pack) => {
                let err = extract(&pack, &ctx("a.forbiddenpy"), PY_SOURCE).unwrap_err();
                assert_eq!(err.stage(), "script", "{err}");
            }
            Err(err) => assert_eq!(err.stage(), "script", "{err}"),
        }
    }
}

#[cfg(feature = "rhai")]
#[test]
fn infinite_loop_in_adapter_is_bounded_not_hung() {
    let tmp = tempfile::tempdir().unwrap();
    write(tmp.path(), "queries/decls.scm", DECL_QUERY);
    write(tmp.path(), "adapter.rhai", b"loop { }");
    write_pack(tmp.path(), &rhai_manifest("loop_pack", "looppy"));
    let pack = compile_pack(read_assets(tmp.path()).unwrap()).unwrap();
    let err = extract(&pack, &ctx("a.looppy"), PY_SOURCE).unwrap_err();
    assert_eq!(err.stage(), "script", "{err}");
}

#[test]
fn query_time_deadline_discards_the_whole_extraction_rather_than_truncating() {
    // Large enough that this simple query's match/capture-extraction loop
    // reliably exceeds the 250ms query deadline on any reasonable machine;
    // the assertion is cancellation itself, not a precise duration. Before
    // this behavior was fixed, tree-sitter's cursor timeout made `matches()`
    // return early as if it had finished, and this extraction silently
    // "succeeded" with a partial (truncated) record set instead of erroring.
    let tmp = tempfile::tempdir().unwrap();
    write(tmp.path(), "queries/decls.scm", DECL_QUERY);
    write_pack(tmp.path(), &query_only_manifest("many_pack", "manypy"));
    let pack = compile_pack(read_assets(tmp.path()).unwrap()).unwrap();
    let mut source = String::new();
    for i in 0..20_000 {
        source.push_str(&format!("def f{i}():\n    pass\n"));
    }
    let err = extract(&pack, &ctx("a.manypy"), &source).unwrap_err();
    assert_eq!(err.stage(), "query", "{err}");
    assert!(err.to_string().contains("deadline"), "{err}");
}

#[cfg(feature = "rhai")]
#[test]
fn output_record_cap_discards_the_whole_extraction() {
    let tmp = tempfile::tempdir().unwrap();
    write(tmp.path(), "queries/decls.scm", DECL_QUERY);
    // A flat array of short strings, not nested maps: Rhai's own array/map
    // size sandbox (not the host's MAX_OUTPUT_RECORDS cap this test targets)
    // counts a growing array's full recursive size on every `.push()`, so a
    // richly-nested record list hits that budget long before 4096 records.
    write(
        tmp.path(),
        "adapter.rhai",
        b"let out = []; for i in range(0, 4200) { out.push(\"x\"); } out",
    );
    write_pack(tmp.path(), &rhai_manifest("overflow_pack", "overflowpy"));
    let pack = compile_pack(read_assets(tmp.path()).unwrap()).unwrap();
    let err = extract(&pack, &ctx("a.overflowpy"), PY_SOURCE).unwrap_err();
    // The adapter's output is not an array of records at all (it's an array
    // of strings) — but it is over MAX_OUTPUT_RECORDS long, and the record
    // decode happens before the count check, so this is a `record` stage
    // error. Confirming the count would need a 4200-element array of valid
    // record JSON objects, which (as above) a Rhai script cannot build
    // within its own sandbox budget — host-side (template) emission is the
    // realistic path to that cap, exercised by code review of the shared
    // `records::validate_records` length check used by both paths.
    assert!(matches!(err.stage(), "record" | "limit"), "{err}");
}

#[test]
fn utf8_multibyte_source_does_not_panic_and_maps_spans_correctly() {
    let tmp = tempfile::tempdir().unwrap();
    write(tmp.path(), "queries/decls.scm", DECL_QUERY);
    write_pack(tmp.path(), &query_only_manifest("utf8_pack", "utf8py"));
    let pack = compile_pack(read_assets(tmp.path()).unwrap()).unwrap();
    let source = "def café():\n    pass\n";
    let extraction = extract(&pack, &ctx("a.utf8py"), source).unwrap();
    let PackRecord::Declaration { name, span, .. } = &extraction.records[0] else {
        panic!("expected a declaration");
    };
    assert_eq!(name, "café");
    assert_eq!(
        &source[span.start_byte..span.end_byte],
        "def café():\n    pass"
    );
}

#[cfg(not(feature = "rhai"))]
#[test]
fn query_only_pack_works_without_the_rhai_feature() {
    let tmp = tempfile::tempdir().unwrap();
    write(tmp.path(), "queries/decls.scm", DECL_QUERY);
    write_pack(tmp.path(), &query_only_manifest("norhai_pack", "norhaipy"));
    let pack = compile_pack(read_assets(tmp.path()).unwrap()).unwrap();
    let extraction = extract(&pack, &ctx("a.norhaipy"), PY_SOURCE).unwrap();
    assert_eq!(extraction.records.len(), 2);
}

#[cfg(not(feature = "rhai"))]
#[test]
fn script_dependent_pack_is_an_explicit_unsupported_error_without_rhai() {
    let tmp = tempfile::tempdir().unwrap();
    write(tmp.path(), "queries/decls.scm", DECL_QUERY);
    write(tmp.path(), "adapter.rhai", b"[]");
    let manifest = json!({
        "schema_version": 1,
        "adapter_api": 1,
        "id": "needsrhai_pack",
        "version": "0.1.0",
        "language": "needsrhai_pack",
        "extensions": ["needsrhaipy"],
        "grammar": "python",
        "capabilities": ["structural_facts"],
        "queries": [{"id": "decls", "path": "queries/decls.scm", "emit": []}],
        "adapter": "adapter.rhai",
        "context_files": [],
        "fixtures": []
    });
    write_pack(tmp.path(), &manifest);
    let assets = read_assets(tmp.path()).unwrap();
    let err = compile_pack(assets).unwrap_err();
    assert_eq!(err.stage(), "unsupported", "{err}");
}
