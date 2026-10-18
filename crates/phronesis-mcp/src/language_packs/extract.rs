//! Compiling a validated pack's queries/script, and running them against one
//! file: `compile_pack` + `extract`
//! (`docs/superpowers/specs/2026-10-17-go-and-language-packs-design.md`
//! §"Record and runtime contract").

use std::collections::HashMap;

use streaming_iterator::StreamingIterator;
use tree_sitter::{Language, Query, QueryCursor, Tree};

use super::error::PackError;
use super::grammar;
use super::manifest::RecordTemplate;
use super::records::{Capture, Extraction, FileContext, PackRecord, QueryMatch, SourceSpan};
use super::templates;
use super::{PackAssets, limits, validate_manifest};

/// One manifest `QuerySpec` after its query file has been compiled against
/// the pack's declared grammar.
#[derive(Debug)]
struct CompiledQuery {
    id: String,
    query: Query,
    emit: Vec<RecordTemplate>,
}

/// An installable pack whose manifest, queries, and (if present) adapter
/// script have all been checked: immutable assets, a content digest, every
/// query compiled, and the script validated (compiled, not run) if the
/// `rhai` feature is available. Construct with [`compile_pack`].
#[derive(Debug)]
pub struct ValidatedPack {
    pub assets: PackAssets,
    pub digest: String,
    compiled_manifest: super::PackManifest,
    compiled_digest: String,
    language: Language,
    queries: Vec<CompiledQuery>,
    script: Option<String>,
}

fn pack_err_field(pack: &str, path: &str, field: &str, message: impl Into<String>) -> PackError {
    PackError::Manifest {
        pack: Some(pack.to_string()),
        path: path.to_string(),
        field: field.to_string(),
        message: message.into(),
    }
}

/// Compile `assets` into a [`ValidatedPack`]: resolve the declared grammar,
/// compile every query, and (when present) validate the adapter script.
/// `read_assets`/`validate_manifest` have already checked the manifest
/// shape and resource paths; this is the next stage, needing the actual
/// bytes.
pub fn compile_pack(assets: PackAssets) -> Result<ValidatedPack, PackError> {
    validate_asset_snapshot(&assets)?;
    let digest = super::digest_assets(&assets);
    let manifest = &assets.manifest;
    let pack_id = manifest.id.clone();

    let language = grammar::grammar_for(&manifest.grammar).ok_or_else(|| {
        if grammar::is_available(&manifest.grammar) {
            PackError::UnknownGrammar {
                pack: Some(pack_id.clone()),
                grammar: manifest.grammar.clone(),
                available: super::BUNDLED_GRAMMARS
                    .iter()
                    .map(|g| g.to_string())
                    .collect(),
            }
        } else {
            PackError::Unsupported {
                pack: Some(pack_id.clone()),
                message: format!(
                    "grammar `{}` is a known bundled grammar, but this binary was built without it",
                    manifest.grammar
                ),
            }
        }
    })?;

    let mut queries = Vec::with_capacity(manifest.queries.len());
    for spec in &manifest.queries {
        let bytes = assets.files.get(&spec.path).ok_or_else(|| {
            pack_err_field(
                &pack_id,
                &spec.path,
                "queries[].path",
                "declared query asset is missing",
            )
        })?;
        let source = std::str::from_utf8(bytes).map_err(|e| PackError::Query {
            pack: Some(pack_id.clone()),
            path: Some(spec.path.clone()),
            message: format!("query file is not valid UTF-8: {e}"),
        })?;
        let query = Query::new(&language, source).map_err(|e| PackError::Query {
            pack: Some(pack_id.clone()),
            path: Some(spec.path.clone()),
            message: e.to_string(),
        })?;
        for (index, template) in spec.emit.iter().enumerate() {
            templates::validate_template(template).map_err(|e| PackError::Manifest {
                pack: Some(pack_id.clone()),
                path: super::MANIFEST_FILE.into(),
                field: format!("queries[{}].emit[{index}]", spec.id),
                message: e.0,
            })?;
        }
        queries.push(CompiledQuery {
            id: spec.id.clone(),
            query,
            emit: spec.emit.clone(),
        });
    }

    let script = match &manifest.adapter {
        Some(path) => {
            let bytes = assets.files.get(path).ok_or_else(|| {
                pack_err_field(
                    &pack_id,
                    path,
                    "adapter",
                    "declared adapter asset is missing",
                )
            })?;
            let source = std::str::from_utf8(bytes).map_err(|e| PackError::Script {
                pack: Some(pack_id.clone()),
                path: Some(path.clone()),
                message: format!("adapter script is not valid UTF-8: {e}"),
            })?;
            validate_adapter_script(&pack_id, path, source)?;
            Some(source.to_string())
        }
        None => None,
    };

    Ok(ValidatedPack {
        compiled_manifest: assets.manifest.clone(),
        compiled_digest: digest.clone(),
        assets,
        digest,
        language,
        queries,
        script,
    })
}

fn validate_asset_snapshot(assets: &PackAssets) -> Result<(), PackError> {
    validate_manifest(&assets.manifest)?;
    let Some(manifest_bytes) = assets.files.get(super::MANIFEST_FILE) else {
        return Err(PackError::Manifest {
            pack: Some(assets.manifest.id.clone()),
            path: super::MANIFEST_FILE.into(),
            field: "manifest".into(),
            message: "manifest bytes are missing from the asset snapshot".into(),
        });
    };
    let parsed: super::PackManifest =
        serde_json::from_slice(manifest_bytes).map_err(|e| PackError::Manifest {
            pack: Some(assets.manifest.id.clone()),
            path: super::MANIFEST_FILE.into(),
            field: "manifest".into(),
            message: e.to_string(),
        })?;
    if parsed != assets.manifest {
        return Err(PackError::Manifest {
            pack: Some(assets.manifest.id.clone()),
            path: super::MANIFEST_FILE.into(),
            field: "manifest".into(),
            message: "parsed manifest differs from the hashed manifest bytes".into(),
        });
    }
    let resources = assets.manifest.resource_paths();
    let mut expected = std::collections::BTreeSet::from([super::MANIFEST_FILE.to_string()]);
    for (_, path) in &resources {
        expected.insert((*path).to_string());
    }
    let actual: std::collections::BTreeSet<_> = assets.files.keys().cloned().collect();
    if let Some(path) = expected.difference(&actual).next() {
        return Err(PackError::Manifest {
            pack: Some(assets.manifest.id.clone()),
            path: path.clone(),
            field: "resources".into(),
            message: "declared asset bytes are missing".into(),
        });
    }
    if let Some(path) = actual.difference(&expected).next() {
        return Err(PackError::Manifest {
            pack: Some(assets.manifest.id.clone()),
            path: path.clone(),
            field: "resources".into(),
            message: "asset is not declared by the manifest".into(),
        });
    }
    if assets.files.len() > limits::MAX_ASSETS {
        return Err(PackError::Limit {
            pack: Some(assets.manifest.id.clone()),
            path: Some(super::MANIFEST_FILE.into()),
            what: "assets".into(),
            max: limits::MAX_ASSETS,
        });
    }
    let mut total = 0usize;
    for (path, bytes) in &assets.files {
        let cap = if path == super::MANIFEST_FILE {
            limits::MAX_MANIFEST_BYTES
        } else if resources
            .iter()
            .any(|(field, name)| *name == path && field.starts_with("queries"))
        {
            limits::MAX_QUERY_BYTES
        } else if assets.manifest.adapter.as_deref() == Some(path.as_str()) {
            limits::MAX_SCRIPT_BYTES
        } else {
            limits::MAX_FILE_BYTES
        };
        if bytes.len() > cap {
            return Err(PackError::Limit {
                pack: Some(assets.manifest.id.clone()),
                path: Some(path.clone()),
                what: "file bytes".into(),
                max: cap,
            });
        }
        total = total.saturating_add(bytes.len());
        if total > limits::MAX_TOTAL_ASSET_BYTES {
            return Err(PackError::Limit {
                pack: Some(assets.manifest.id.clone()),
                path: None,
                what: "total asset bytes".into(),
                max: limits::MAX_TOTAL_ASSET_BYTES,
            });
        }
    }
    Ok(())
}

#[cfg(feature = "rhai")]
fn validate_adapter_script(pack: &str, path: &str, source: &str) -> Result<(), PackError> {
    phronesis_rhai::validate_adapter(source).map_err(|e| PackError::Script {
        pack: Some(pack.to_string()),
        path: Some(path.to_string()),
        message: e.to_string(),
    })
}

#[cfg(not(feature = "rhai"))]
fn validate_adapter_script(pack: &str, _path: &str, _source: &str) -> Result<(), PackError> {
    Err(PackError::Unsupported {
        pack: Some(pack.to_string()),
        message: "this pack declares an adapter script, which requires the rhai feature".into(),
    })
}

#[cfg(feature = "rhai")]
fn run_adapter_script(
    pack: &str,
    path: &str,
    script: &str,
    input: &serde_json::Value,
    max_output_records: usize,
) -> Result<serde_json::Value, PackError> {
    phronesis_rhai::evaluate_adapter_with_output_limit(script, input, max_output_records).map_err(
        |e| match e {
            phronesis_rhai::AdapterError::OutputLimit(max) => PackError::Limit {
                pack: Some(pack.to_string()),
                path: Some(path.to_string()),
                what: "output records".into(),
                max,
            },
            other => PackError::Script {
                pack: Some(pack.to_string()),
                path: Some(path.to_string()),
                message: other.to_string(),
            },
        },
    )
}

#[cfg(not(feature = "rhai"))]
fn run_adapter_script(
    pack: &str,
    _path: &str,
    _script: &str,
    _input: &serde_json::Value,
    _max_output_records: usize,
) -> Result<serde_json::Value, PackError> {
    Err(PackError::Unsupported {
        pack: Some(pack.to_string()),
        message: "this pack declares an adapter script, which requires the rhai feature".into(),
    })
}

/// Run `pack`'s compiled queries (and, if present, its adapter script) over
/// `source`, producing the records and diagnostics for `context.file_rel`.
/// A parse failure, a query/script error, or a limit breach returns `Err`
/// and discards the extraction entirely — never a partial successful
/// result.
pub fn extract(
    pack: &ValidatedPack,
    context: &FileContext,
    source: &str,
) -> Result<Extraction, PackError> {
    let pack_id = pack.assets.manifest.id.clone();
    let file_rel = context.file_rel.as_str();

    if pack.assets.manifest != pack.compiled_manifest
        || pack.digest != pack.compiled_digest
        || pack.digest != super::digest_assets(&pack.assets)
    {
        return Err(PackError::Manifest {
            pack: Some(pack.compiled_manifest.id.clone()),
            path: super::MANIFEST_FILE.into(),
            field: "assets".into(),
            message: "validated pack assets changed after compilation".into(),
        });
    }
    if source.len() > limits::MAX_FILE_BYTES {
        return Err(PackError::Limit {
            pack: Some(pack_id.clone()),
            path: Some(file_rel.to_string()),
            what: "source bytes".into(),
            max: limits::MAX_FILE_BYTES,
        });
    }
    if context.context_files.len() > limits::MAX_CONTEXT_FILES {
        return Err(PackError::Limit {
            pack: Some(pack_id.clone()),
            path: Some(file_rel.to_string()),
            what: "context files".into(),
            max: limits::MAX_CONTEXT_FILES,
        });
    }
    for context_file in &context.context_files {
        if context_file.content.len() > limits::MAX_FILE_BYTES {
            return Err(PackError::Limit {
                pack: Some(pack_id.clone()),
                path: Some(context_file.file_rel.clone()),
                what: "context bytes".into(),
                max: limits::MAX_FILE_BYTES,
            });
        }
    }

    let tree = grammar::parse(&pack.language, source).ok_or_else(|| PackError::Query {
        pack: Some(pack_id.clone()),
        path: Some(file_rel.to_string()),
        message: "parsing failed or exceeded the deadline".into(),
    })?;
    if tree.root_node().has_error() {
        return Err(PackError::Query {
            pack: Some(pack_id.clone()),
            path: Some(file_rel.to_string()),
            message: "source contains syntax errors or missing syntax nodes".into(),
        });
    }

    let matches = run_queries(pack, file_rel, source, &tree)?;

    let mut by_query: HashMap<&str, Vec<&QueryMatch>> = HashMap::new();
    for m in &matches {
        by_query.entry(m.query.as_str()).or_default().push(m);
    }

    let mut records = Vec::new();
    let mut diagnostics = Vec::new();
    for compiled in &pack.queries {
        if compiled.emit.is_empty() {
            continue;
        }
        let Some(query_matches) = by_query.get(compiled.id.as_str()) else {
            continue;
        };
        for m in query_matches {
            for template in &compiled.emit {
                match templates::render_template(&pack_id, file_rel, template, m) {
                    Ok(value) => match serde_json::from_value::<PackRecord>(value) {
                        Ok(record) => {
                            ensure_record_capacity(&pack_id, file_rel, records.len())?;
                            records.push(record);
                        }
                        Err(e) => {
                            return Err(PackError::Record {
                                pack: Some(pack_id.clone()),
                                path: Some(file_rel.to_string()),
                                message: format!("rendered template is not a valid record: {e}"),
                            });
                        }
                    },
                    Err(diagnostic) => diagnostics.push(diagnostic),
                }
            }
        }
    }

    if let Some(script) = &pack.script {
        let input = serde_json::json!({
            "context": context,
            "matches": matches,
        });
        let remaining = limits::MAX_OUTPUT_RECORDS.saturating_sub(records.len());
        let output = run_adapter_script(&pack_id, file_rel, script, &input, remaining)?;
        let extra: Vec<PackRecord> =
            serde_json::from_value(output).map_err(|e| PackError::Record {
                pack: Some(pack_id.clone()),
                path: Some(file_rel.to_string()),
                message: format!("adapter script output is not a valid record array: {e}"),
            })?;
        records.extend(extra);
    }

    if records.len() > limits::MAX_OUTPUT_RECORDS {
        return Err(PackError::Limit {
            pack: Some(pack_id.clone()),
            path: Some(file_rel.to_string()),
            what: "output records".into(),
            max: limits::MAX_OUTPUT_RECORDS,
        });
    }
    super::records::validate_records(&pack_id, file_rel, &records, source)?;

    Ok(Extraction {
        records,
        diagnostics,
    })
}

fn ensure_record_capacity(pack: &str, file_rel: &str, count: usize) -> Result<(), PackError> {
    if count >= limits::MAX_OUTPUT_RECORDS {
        return Err(PackError::Limit {
            pack: Some(pack.to_string()),
            path: Some(file_rel.to_string()),
            what: "output records".into(),
            max: limits::MAX_OUTPUT_RECORDS,
        });
    }
    Ok(())
}

/// Run every compiled query of `pack` over an already-parsed `tree`, bounded
/// by the match, capture, and capture-text caps in [`limits`]. A limit
/// breach discards the whole extraction (returns `Err`) rather than
/// truncating it into a successful partial result.
fn run_queries(
    pack: &ValidatedPack,
    file_rel: &str,
    source: &str,
    tree: &Tree,
) -> Result<Vec<QueryMatch>, PackError> {
    let pack_id = &pack.assets.manifest.id;
    let mut matches = Vec::new();
    let mut total_captures: usize = 0;
    let mut total_capture_bytes: usize = 0;

    for compiled in &pack.queries {
        let mut cursor = QueryCursor::new();
        cursor.set_match_limit(limits::MAX_MATCHES as u32);
        let capture_names = compiled.query.capture_names();

        // `set_timeout_micros` stops `matches()` silently early: the
        // iterator just runs dry, which this code would otherwise read as
        // "no more matches" and return a truncated result as if it were
        // complete — exactly the "limit exhaustion must discard, never
        // truncate" rule this module exists to uphold. The progress
        // callback instead lets us tell "timed out" apart from "finished".
        let deadline =
            std::time::Instant::now() + std::time::Duration::from_millis(limits::QUERY_DEADLINE_MS);
        let mut timed_out = false;
        let mut progress = |_state: &tree_sitter::QueryCursorState| -> bool {
            if std::time::Instant::now() >= deadline {
                timed_out = true;
                true
            } else {
                false
            }
        };
        let options = tree_sitter::QueryCursorOptions::new().progress_callback(&mut progress);
        let mut it = cursor.matches_with_options(
            &compiled.query,
            tree.root_node(),
            source.as_bytes(),
            options,
        );
        while let Some(m) = it.next() {
            let mut captures = Vec::with_capacity(m.captures.len());
            for cap in m.captures {
                total_captures += 1;
                if total_captures > limits::MAX_CAPTURES {
                    return Err(PackError::Limit {
                        pack: Some(pack_id.clone()),
                        path: Some(file_rel.to_string()),
                        what: "query captures".into(),
                        max: limits::MAX_CAPTURES,
                    });
                }
                let text = cap
                    .node
                    .utf8_text(source.as_bytes())
                    .map_err(|e| PackError::Query {
                        pack: Some(pack_id.clone()),
                        path: Some(file_rel.to_string()),
                        message: format!("capture text is not valid UTF-8: {e}"),
                    })?;
                total_capture_bytes += text.len();
                if total_capture_bytes > limits::MAX_CAPTURE_TEXT_BYTES {
                    return Err(PackError::Limit {
                        pack: Some(pack_id.clone()),
                        path: Some(file_rel.to_string()),
                        what: "captured text bytes".into(),
                        max: limits::MAX_CAPTURE_TEXT_BYTES,
                    });
                }
                let span =
                    SourceSpan::from_bytes(source, cap.node.start_byte(), cap.node.end_byte())
                        .ok_or_else(|| PackError::Query {
                            pack: Some(pack_id.clone()),
                            path: Some(file_rel.to_string()),
                            message: "capture span is not a valid range of the source".into(),
                        })?;
                captures.push(Capture {
                    name: capture_names
                        .get(cap.index as usize)
                        .copied()
                        .unwrap_or("")
                        .to_string(),
                    kind: cap.node.kind().to_string(),
                    text: text.to_string(),
                    span,
                });
            }
            matches.push(QueryMatch {
                query: compiled.id.clone(),
                captures,
            });
            if matches.len() > limits::MAX_MATCHES {
                return Err(PackError::Limit {
                    pack: Some(pack_id.clone()),
                    path: Some(file_rel.to_string()),
                    what: "query matches".into(),
                    max: limits::MAX_MATCHES,
                });
            }
        }
        drop(it);
        if timed_out {
            return Err(PackError::Query {
                pack: Some(pack_id.clone()),
                path: Some(file_rel.to_string()),
                message: format!(
                    "query exceeded its {}ms deadline",
                    limits::QUERY_DEADLINE_MS
                ),
            });
        }
        if cursor.did_exceed_match_limit() {
            return Err(PackError::Limit {
                pack: Some(pack_id.clone()),
                path: Some(file_rel.to_string()),
                what: "query matches".into(),
                max: limits::MAX_MATCHES,
            });
        }
    }
    Ok(matches)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::language_packs::read_assets;
    use serde_json::{Value, json};
    use std::fs;
    use std::path::Path;

    const DECL_QUERY: &[u8] = b"(function_definition name: (identifier) @name) @def";
    const PY_SOURCE: &str = "def a():\n    pass\n\n\ndef b():\n    pass\n";

    fn manifest(id: &str, ext: &str) -> Value {
        json!({
            "schema_version": 1,
            "adapter_api": 1,
            "id": id,
            "version": "0.1.0",
            "language": id,
            "extensions": [ext],
            "grammar": "python",
            "capabilities": ["structural_facts"],
            "queries": [{"id": "decls", "path": "queries/decls.scm", "emit": [
                {"kind": "declaration", "name": "${name}", "scope": "", "span_capture": "def"}
            ]}],
            "context_files": [],
            "fixtures": []
        })
    }

    fn write_pack(dir: &Path, manifest: &Value) {
        write(dir, "pack.json", manifest.to_string().as_bytes());
    }

    fn write(dir: &Path, rel: &str, bytes: &[u8]) {
        let path = dir.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    /// A minimal query-only pack over the bundled Python grammar: one
    /// `function_definition` query emitting a `declaration` record per
    /// match.
    fn demo_pack(tmp: &Path) -> ValidatedPack {
        let manifest = serde_json::json!({
            "schema_version": 1,
            "adapter_api": 1,
            "id": "demo_py",
            "version": "0.1.0",
            "language": "demo_py",
            "extensions": ["demopy"],
            "grammar": "python",
            "capabilities": ["structural_facts"],
            "queries": [{
                "id": "decls",
                "path": "queries/decls.scm",
                "emit": [{"kind": "declaration", "name": "${name}", "scope": "", "span_capture": "def"}]
            }],
            "context_files": [],
            "fixtures": []
        });
        write(
            tmp,
            "queries/decls.scm",
            b"(function_definition name: (identifier) @name) @def",
        );
        write(tmp, "pack.json", manifest.to_string().as_bytes());
        let assets = read_assets(tmp).unwrap();
        compile_pack(assets).unwrap()
    }

    fn ctx(file_rel: &str) -> FileContext {
        FileContext {
            file_rel: file_rel.to_string(),
            context_files: Vec::new(),
        }
    }

    #[test]
    fn query_only_pack_emits_more_than_one_hundred_declarations_completely() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "queries/decls.scm", DECL_QUERY);
        write_pack(tmp.path(), &manifest("many_pack", "manypy"));
        let pack = compile_pack(read_assets(tmp.path()).unwrap()).unwrap();
        let source = (0..128)
            .map(|i| format!("def f{i}():\n    pass\n"))
            .collect::<String>();
        let extraction = extract(&pack, &ctx("a.manypy"), &source).unwrap();
        assert_eq!(extraction.records.len(), 128);
        assert!(extraction.diagnostics.is_empty());
    }

    #[test]
    fn malformed_source_and_oversized_inputs_fail_before_querying() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "queries/decls.scm", DECL_QUERY);
        write_pack(tmp.path(), &manifest("guard_pack", "guardpy"));
        let pack = compile_pack(read_assets(tmp.path()).unwrap()).unwrap();
        let err = extract(&pack, &ctx("bad.py"), "def broken(:\n    pass\n").unwrap_err();
        assert_eq!(err.stage(), "query");
        assert!(err.to_string().contains("syntax"));

        let err = extract(&pack, &ctx("large.py"), &" ".repeat(1024 * 1024 + 1)).unwrap_err();
        assert_eq!(err.stage(), "limit");
        assert!(err.to_string().contains("source bytes"));

        let mut context = ctx("context.py");
        context
            .context_files
            .push(crate::language_packs::ContextFile {
                name: "config".into(),
                file_rel: "config".into(),
                directory_rel: ".".into(),
                content: "x".repeat(1024 * 1024 + 1),
            });
        let err = extract(&pack, &context, "").unwrap_err();
        assert_eq!(err.stage(), "limit");
        assert!(err.to_string().contains("context bytes"));
    }

    #[test]
    fn invalid_template_schema_fails_at_compile_and_extraction() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "queries/decls.scm", DECL_QUERY);
        let mut m = manifest("bad_template", "badtemplatepy");
        m["queries"][0]["emit"][0]["name"] = json!(17);
        write_pack(tmp.path(), &m);
        let err = compile_pack(read_assets(tmp.path()).unwrap()).unwrap_err();
        assert_eq!(err.stage(), "manifest");
        assert!(err.to_string().contains("emit[0]"));

        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "queries/decls.scm", DECL_QUERY);
        write_pack(tmp.path(), &manifest("runtime_template", "runtimepy"));
        let mut pack = compile_pack(read_assets(tmp.path()).unwrap()).unwrap();
        pack.queries[0].emit[0].0.insert("name".into(), json!(17));
        let err = extract(&pack, &ctx("a.runtimepy"), PY_SOURCE).unwrap_err();
        assert_eq!(err.stage(), "record");
        assert!(err.to_string().contains("valid record"));
    }

    #[test]
    fn compiled_assets_cannot_be_changed_after_validation() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "queries/decls.scm", DECL_QUERY);
        write_pack(tmp.path(), &manifest("snapshot_pack", "snapshotpy"));

        let mut pack = compile_pack(read_assets(tmp.path()).unwrap()).unwrap();
        pack.assets.manifest.id = "forged_pack".into();
        let err = extract(&pack, &ctx("a.snapshotpy"), PY_SOURCE).unwrap_err();
        assert_eq!(err.stage(), "manifest");

        let mut pack = compile_pack(read_assets(tmp.path()).unwrap()).unwrap();
        pack.assets
            .files
            .insert("queries/decls.scm".into(), b"((identifier) @name)".to_vec());
        let err = extract(&pack, &ctx("a.snapshotpy"), PY_SOURCE).unwrap_err();
        assert_eq!(err.stage(), "manifest");

        let mut pack = compile_pack(read_assets(tmp.path()).unwrap()).unwrap();
        pack.digest.push('0');
        let err = extract(&pack, &ctx("a.snapshotpy"), PY_SOURCE).unwrap_err();
        assert_eq!(err.stage(), "manifest");
    }

    #[test]
    fn query_template_output_cap_is_enforced_while_emitting() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "queries/decls.scm", DECL_QUERY);
        let mut m = manifest("overflow_pack", "overflowpy");
        let template = json!({
            "kind": "declaration", "name": "${name}", "scope": "", "span_capture": "def"
        });
        m["queries"][0]["emit"] = Value::Array(vec![template; 700]);
        write_pack(tmp.path(), &m);
        let pack = compile_pack(read_assets(tmp.path()).unwrap()).unwrap();
        let source = (0..6)
            .map(|i| format!("def f{i}():\n    pass\n"))
            .collect::<String>();
        let err = extract(&pack, &ctx("a.overflowpy"), &source).unwrap_err();
        assert_eq!(err.stage(), "limit", "{err}");
        assert!(err.to_string().contains("output records"));
    }

    #[test]
    fn query_only_pack_emits_declarations_for_every_function() {
        let tmp = tempfile::tempdir().unwrap();
        let pack = demo_pack(tmp.path());
        let source = "def a():\n    pass\n\n\ndef b():\n    pass\n";
        let extraction = extract(&pack, &ctx("a.demopy"), source).unwrap();
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
                _ => panic!("expected declaration, got {r:?}"),
            })
            .collect();
        assert_eq!(names, ["a", "b"]);
    }

    #[test]
    fn comments_and_strings_produce_no_declarations() {
        let tmp = tempfile::tempdir().unwrap();
        let pack = demo_pack(tmp.path());
        let source = "# def lookalike():\n#     pass\nx = \"def also_not_a_function():\"\n";
        let extraction = extract(&pack, &ctx("a.demopy"), source).unwrap();
        assert!(extraction.records.is_empty(), "{:?}", extraction.records);
    }

    #[test]
    fn malformed_source_does_not_fabricate_a_result() {
        // Python's grammar is error-tolerant (it still returns a tree with
        // ERROR nodes), so "malformed" here means an encoding the grammar's
        // language cannot represent as valid UTF-8 byte offsets is not
        // reachable from a `&str`; the behavior we pin instead is that
        // genuinely nonsensical source still parses without panicking and
        // yields no fabricated declarations.
        let tmp = tempfile::tempdir().unwrap();
        let pack = demo_pack(tmp.path());
        let err = extract(&pack, &ctx("a.demopy"), "def (((((").unwrap_err();
        assert_eq!(err.stage(), "query", "{err}");
    }

    #[test]
    #[cfg(not(feature = "go-grammar"))]
    fn unsupported_grammar_feature_is_reported_as_unsupported_not_unknown() {
        let tmp = tempfile::tempdir().unwrap();
        let manifest = serde_json::json!({
            "schema_version": 1,
            "adapter_api": 1,
            "id": "demo_go",
            "version": "0.1.0",
            "language": "demo_go",
            "extensions": ["demogo"],
            "grammar": "go",
            "capabilities": ["structural_facts"],
            "queries": [{"id": "decls", "path": "queries/decls.scm", "emit": []}],
            "context_files": [],
            "fixtures": []
        });
        write(tmp.path(), "queries/decls.scm", b"(_)");
        write(tmp.path(), "pack.json", manifest.to_string().as_bytes());
        let assets = read_assets(tmp.path()).unwrap();
        let err = compile_pack(assets).unwrap_err();
        assert_eq!(err.stage(), "unsupported", "{err}");
    }
}
