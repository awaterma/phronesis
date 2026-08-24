use super::lexer::{QType, Tok, Trim, lex, lex_action};
use super::*;

fn ctx(_chart_name: &str) -> UnitContext {
    UnitContext {
        id: "helm3:mychart".to_string(),
        module_base: String::new(),
        siblings: std::collections::BTreeMap::new(),
        ts: crate::graph::unit::TsConfig::default(),
        files: Vec::new(),
        lua_files: Vec::new(),
        cue_files: Vec::new(),
        test_target: false,
    }
}

fn edges_of(x: &Extracted, p: &str) -> Vec<Vec<String>> {
    x.edges
        .iter()
        .filter(|e| e.p == p)
        .map(|e| e.a.clone())
        .collect()
}

// ── Lexer tests ──────────────────────────────────────────────────

#[test]
fn lexer_detects_open_close_actions() {
    let toks = lex("hello {{ world }} world");
    assert!(toks.iter().any(|t| matches!(t, Tok::OpenAction(_))));
    assert!(toks.iter().any(|t| matches!(t, Tok::CloseAction(_))));
}

#[test]
fn lexer_detects_trim_markers() {
    let toks = lex("{{- define \"x\" -}}");
    assert!(
        toks.iter()
            .any(|t| matches!(t, Tok::OpenAction(Trim::Left)))
    );
}

#[test]
fn lexer_captures_quoted_strings() {
    let toks = lex_action(r#"define "mychart.helpers" ."#);
    assert!(
        toks.iter()
            .any(|t| matches!(t, Tok::QStr(QType::Dbl, s) if s == r#""mychart.helpers""#))
    );
}

#[test]
fn lexer_captures_raw_strings() {
    let toks = lex_action(r#"`config.yaml`"#);
    assert!(
        toks.iter()
            .any(|t| matches!(t, Tok::QStr(QType::Raw, s) if s == "`config.yaml`"))
    );
}

#[test]
fn lexer_captures_single_quoted_strings() {
    let toks = lex_action(r#"'single quoted'"#);
    assert!(
        toks.iter()
            .any(|t| matches!(t, Tok::QStr(QType::Sgl, s) if s == "'single quoted'"))
    );
}

// ── File classifier tests ─────────────────────────────────────────

#[test]
fn non_helm_files_return_empty() {
    let out = extract_helm3("foo.py", "pass\n", &ctx("charts/app"), None);
    assert!(out.edges.is_empty());
    assert!(!out.parse_failed);
}

#[test]
fn chart_yaml_is_recognized() {
    let out = extract_helm3(
        "Chart.yaml",
        "apiVersion: v2\nname: mychart\n",
        &ctx("charts/app"),
        Some("charts/app"),
    );
    assert!(!out.edges.is_empty());
    let fts = edges_of(&out, "file_type");
    assert!(fts.iter().any(|a| a[1] == "chart_manifest"));
}

#[test]
fn tpl_file_is_recognized() {
    let out = extract_helm3(
        "templates/deployment.tpl",
        "{{ define \"deployment\" }}\napiVersion: apps/v1\n{{ end }}\n",
        &ctx("charts/app"),
        Some("charts/app"),
    );
    assert!(!out.edges.is_empty());
    let fts = edges_of(&out, "file_type");
    assert!(fts.iter().any(|a| a[1] == "helm_template"));
}

#[test]
fn empty_content_returns_unparseable() {
    let out = extract_helm3(
        "templates/deployment.tpl",
        "",
        &ctx("charts/app"),
        Some("charts/app"),
    );
    assert!(out.parse_failed);
    assert!(out.edges.is_empty());
}

#[test]
fn whitespace_only_content_returns_unparseable() {
    let out = extract_helm3(
        "templates/deployment.tpl",
        "   \n  \n  \n  ",
        &ctx("charts/app"),
        Some("charts/app"),
    );
    assert!(out.parse_failed);
}

// ── Define / block tests ──────────────────────────────────────────

#[test]
fn a_define_becomes_graph_definition() {
    let out = extract_helm3(
        "templates/deployment.tpl",
        "{{ define \"mychart.deployment\" }}\napiVersion: apps/v1\n{{ end }}\n",
        &ctx("charts/app"),
        Some("charts/app"),
    );
    let defs = edges_of(&out, "defines");
    assert_eq!(defs.len(), 1);
    assert!(defs[0][1].contains("::define:"));
    assert!(defs[0][1].contains("deployment"));
}

#[test]
fn multiple_defines_in_one_file() {
    let content = r#"
{{ define "chart.helpers" }}
helpers here
{{ end }}

{{ define "chart.tpl" }}
more
{{ end }}
"#;
    let out = extract_helm3(
        "templates/_helpers.tpl",
        content,
        &ctx("charts/app"),
        Some("charts/app"),
    );
    let defs = edges_of(&out, "defines");
    assert_eq!(defs.len(), 2);
}

#[test]
fn block_creates_define_and_import() {
    let out = extract_helm3(
        "templates/deployment.tpl",
        "{{ block \"mychart.tpl\" . }}\n{{ end }}\n",
        &ctx("charts/app"),
        Some("charts/app"),
    );
    let defs = edges_of(&out, "defines");
    assert_eq!(defs.len(), 1);
    let imps = edges_of(&out, "imports");
    assert!(imps.iter().any(|a| a[1].contains("mychart.tpl")));
}

#[test]
fn helpers_tpl_is_classified_correctly() {
    let out = extract_helm3(
        "templates/_helpers.tpl",
        "{{ define \"helpers\" }}\n{{ end }}\n",
        &ctx("charts/app"),
        Some("charts/app"),
    );
    let fts = edges_of(&out, "file_type");
    assert!(fts.iter().any(|a| a[1] == "helm_helpers"));
}

#[test]
fn declares_module_edge_maps_file_to_module() {
    let out = extract_helm3(
        "templates/deployment.tpl",
        "{{ define \"x\" }}\n{{ end }}\n",
        &ctx("charts/app"),
        Some("charts/app"),
    );
    let decls = edges_of(&out, "declares_module");
    assert_eq!(decls.len(), 1);
    assert_eq!(decls[0][0], "templates/deployment.tpl");
    assert!(decls[0][1].starts_with("helm3:mychart::"));
    assert!(decls[0][1].contains("templates"));
}

// ── Template / include call tests ─────────────────────────────────

#[test]
fn template_call_becomes_imports_edge() {
    let out = extract_helm3(
        "templates/deployment.tpl",
        "{{ template \"mychart.helpers\" }}\n",
        &ctx("charts/app"),
        Some("charts/app"),
    );
    let imps = edges_of(&out, "imports");
    assert!(
        imps.iter()
            .any(|a| { a[0].contains("deployment") && a[1].contains("helpers") })
    );
}

#[test]
fn include_call_becomes_imports_edge() {
    let out = extract_helm3(
        "templates/deployment.tpl",
        "{{ include \"mychart.tpl\" . }}\n",
        &ctx("charts/app"),
        Some("charts/app"),
    );
    let imps = edges_of(&out, "imports");
    assert!(
        imps.iter()
            .any(|a| { a[0].contains("deployment") && a[1].contains("tpl") })
    );
}

// ── .Values tests ─────────────────────────────────────────────────

#[test]
fn values_reference_becomes_imports_edge() {
    let out = extract_helm3(
        "templates/deployment.tpl",
        "image: {{ .Values.image.repository }}:{{ .Values.image.tag }}\n",
        &ctx("charts/app"),
        Some("charts/app"),
    );
    let imps = edges_of(&out, "imports");
    assert!(imps.iter().any(|a| {
        a[0].contains("deployment") && a[1].contains("image") && a[1].contains("values")
    }));
}

#[test]
fn values_references_are_deduplicated() {
    let content = r#"
{{ .Values.image.repository }}
{{ .Values.image.repository }}
{{ .Values.image.tag }}
"#;
    let out = extract_helm3(
        "templates/deployment.tpl",
        content,
        &ctx("charts/app"),
        Some("charts/app"),
    );
    let imps = edges_of(&out, "imports");
    let value_imports: Vec<&Vec<String>> = imps
        .iter()
        .filter(|a| a[0].contains("deployment"))
        .collect();
    assert_eq!(value_imports.len(), 2);
}

// ── .Files.Get tests ──────────────────────────────────────────────

#[test]
fn file_reference_resolves_to_yaml_import() {
    let out = extract_helm3(
        "templates/deployment.tpl",
        "{{ .Files.Get \"files/config.yaml\" }}\n",
        &ctx("charts/app"),
        Some("charts/app"),
    );
    let imps = edges_of(&out, "imports");
    assert!(imps.iter().any(|a| {
        a[0].contains("deployment") && a[1].contains("config") && a[1].starts_with("yaml")
    }));
}

#[test]
fn file_reference_to_txt_is_not_exported() {
    let out = extract_helm3(
        "templates/deployment.tpl",
        "{{ .Files.Get \"files/readme.txt\" }}\n",
        &ctx("charts/app"),
        Some("charts/app"),
    );
    let imps = edges_of(&out, "imports");
    assert!(!imps.iter().any(|a| a[1].contains("readme")));
}

// ── Risk watchlist tests ──────────────────────────────────────────

#[test]
fn tpl_call_detected_as_risk() {
    let out = extract_helm3(
        "templates/deployment.tpl",
        "{{ tpl .Files.Get \"sub.tpl\" . }}\n",
        &ctx("charts/app"),
        Some("charts/app"),
    );
    let tpl = edges_of(&out, "helm3_dynamic_tpl");
    assert!(!tpl.is_empty());
}

#[test]
fn lookup_call_detected_as_risk() {
    let out = extract_helm3(
        "templates/deployment.tpl",
        "{{ lookup \"apps/v1\" \"Deployment\" \"default\" \"my-deploy\" }}\n",
        &ctx("charts/app"),
        Some("charts/app"),
    );
    let lk = edges_of(&out, "helm3_cluster_lookup");
    assert!(!lk.is_empty());
}

// ── Negative tests — calls inside comments should not be extracted ─

#[test]
fn define_inside_comment_is_not_extracted() {
    let out = extract_helm3(
        "templates/deployment.tpl",
        "{{/* {{ define \"should_not_appear\" }} */}}\n",
        &ctx("charts/app"),
        Some("charts/app"),
    );
    let defs = edges_of(&out, "defines");
    assert!(
        !defs.iter().any(|d| d[1].contains("should_not_appear")),
        "define inside comment must not be extracted"
    );
}

#[test]
fn template_call_inside_comment_is_not_extracted() {
    let out = extract_helm3(
        "templates/deployment.tpl",
        "{{/* {{ template \"ghost\" . }} */}}\n",
        &ctx("charts/app"),
        Some("charts/app"),
    );
    let imps = edges_of(&out, "imports");
    assert!(
        !imps.iter().any(|i| i[1].contains("ghost")),
        "template call inside comment must not be extracted"
    );
}

#[test]
fn tpl_call_inside_comment_is_not_extracted() {
    let out = extract_helm3(
        "templates/deployment.tpl",
        "{{/* tpl .Files.Get \"injected.tpl\" . */}}\n",
        &ctx("charts/app"),
        Some("charts/app"),
    );
    let tpl = edges_of(&out, "helm3_dynamic_tpl");
    assert!(tpl.is_empty(), "tpl inside comment must not be detected");
}

#[test]
fn lookup_call_inside_comment_is_not_extracted() {
    let out = extract_helm3(
        "templates/deployment.tpl",
        "{{/* lookup \"apps/v1\" \"Deployment\" \"ns\" \"name\" */}}\n",
        &ctx("charts/app"),
        Some("charts/app"),
    );
    let lk = edges_of(&out, "helm3_cluster_lookup");
    assert!(lk.is_empty(), "lookup inside comment must not be detected");
}

#[test]
fn values_inside_comment_not_extracted() {
    let out = extract_helm3(
        "templates/deployment.tpl",
        "{{/* {{ .Values.ghost.value }} */}}\n",
        &ctx("charts/app"),
        Some("charts/app"),
    );
    let imps = edges_of(&out, "imports");
    assert!(
        !imps.iter().any(|i| i[1].contains("ghost")),
        ".Values inside comment must not be extracted"
    );
}

// ── Quoted string tests — calls inside quoted strings ─────────────

#[test]
fn define_inside_double_quoted_string_is_not_extracted() {
    let out = extract_helm3(
        "templates/deployment.tpl",
        "{{ $msg := \"define should_not_appear\" }}\n",
        &ctx("charts/app"),
        Some("charts/app"),
    );
    let defs = edges_of(&out, "defines");
    assert!(
        !defs.iter().any(|d| d[1].contains("should_not_appear")),
        "define inside string must not be extracted"
    );
}

// ── Whitespace trim marker tests ──────────────────────────────────

#[test]
fn edge_case_define_with_dash_trim() {
    let out = extract_helm3(
        "templates/deployment.tpl",
        "{{- define \"mychart.dash\" }}\n{{ end }}\n",
        &ctx("charts/app"),
        Some("charts/app"),
    );
    let defs = edges_of(&out, "defines");
    assert_eq!(defs.len(), 1);
}

// ── Module path tests ─────────────────────────────────────────────

#[test]
fn module_path_for_helpers_file() {
    let path = build_module_path("templates/_helpers.tpl", "mychart");
    assert!(path.contains("_helpers"));
    assert!(path.starts_with("helm3:mychart::"));
}

#[test]
fn module_path_for_chart_yaml() {
    let path = build_module_path("Chart.yaml", "mychart");
    assert!(path.contains("Chart"));
    assert!(path.starts_with("helm3:mychart::"));
}

#[test]
fn module_path_for_nested_template() {
    let path = build_module_path("templates/subdir/deployment.tpl", "mychart");
    assert!(path.contains("subdir"));
    assert!(path.contains("deployment"));
}

// ── Pipeline / nested action tests ────────────────────────────────

#[test]
fn pipeline_with_multiple_actions() {
    let out = extract_helm3(
        "templates/deployment.tpl",
        "{{ include \"tpl\" . }}\n{{- define \"a\" -}}\n{{ template \"b\" . }}\n{{ end }}\n",
        &ctx("charts/app"),
        Some("charts/app"),
    );
    let imps = edges_of(&out, "imports");
    assert!(imps.iter().any(|a| a[1].contains("tpl")));
    assert!(imps.iter().any(|a| a[1].contains("b")));
}

#[test]
fn if_scope_does_not_interfere_with_define() {
    let out = extract_helm3(
        "templates/deployment.tpl",
        "{{- if .Values.enabled }}\n{{ define \"mychart.feature\" }}\n{{ end }}\n{{- end }}\n",
        &ctx("charts/app"),
        Some("charts/app"),
    );
    let defs = edges_of(&out, "defines");
    assert_eq!(defs.len(), 1);
    assert!(defs[0][1].contains("feature"));
}

// ── Malformed action tests ────────────────────────────────────────

#[test]
fn unclosed_action_does_not_collapse_graph_state() {
    // Malformed {{ define "x" with no closing }} should not panic.
    let out = extract_helm3(
        "templates/deployment.tpl",
        "{{ define \"partial\"\n",
        &ctx("charts/app"),
        Some("charts/app"),
    );
    // Unclosed action shouldn't extract anything, but must not panic.
    let defs = edges_of(&out, "defines");
    assert!(defs.is_empty(), "unclosed action should not extract");
}

#[test]
fn single_curly_brace_is_not_action() {
    let out = extract_helm3(
        "templates/deployment.yaml",
        "{\n  \"apiVersion\": \"apps/v1\"\n}\n",
        &ctx("charts/app"),
        Some("charts/app"),
    );
    let imps = edges_of(&out, "imports");
    assert!(
        imps.is_empty(),
        "curly braces in YAML must not trigger actions"
    );
}
