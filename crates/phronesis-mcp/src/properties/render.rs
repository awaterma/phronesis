//! The render step of SPEC-verification-artifact-generation.md: property
//! record → Rhai template (dedicated render entry) → S5 body validation →
//! `verification/unreviewed/<artifact>` → (a later invocation) the confined
//! executor → a bound result record (D9).
//!
//! Division of labor (spec "Rhai renders, Rust acts"): the template only
//! turns a read-only property record and a frozen, sorted input set into one
//! string. Everything that decides — the S1 opt-in, the S2 status read from
//! the record, template lookup, escaping, validation, containment, the
//! journal, the review gate, execution — is host code in this module.
//!
//! Trust: templates come from `verification/templates/` (a trust anchor the
//! `llm` pack refuses agent writes to). `verification/template-drafts/` is
//! consulted only when the caller passes `allow_drafts` (a dev flag), and a
//! result produced from a draft is recorded with `template_origin:
//! "template_drafts"`, which hydration never binds as verified evidence.

use std::path::{Path, PathBuf};

use serde::Serialize;
use thiserror::Error;

use crate::action_log::{self, LogEntry};
use crate::properties::execute::{self, ExecutionError, artifact_sha256};
use crate::properties::store::{
    Encoding, Property, PropertyStatus, PropertyStoreError, ResultRecord, load_properties,
};
use crate::properties::validate::{BodyValidationError, escape_rust_string_literal, validate_body};

/// Trusted, human-principal-owned templates (S1/S5 trust anchor).
pub const TEMPLATES_DIR: &str = "verification/templates";
/// Untrusted template drafts — consulted only behind `allow_drafts`.
pub const TEMPLATE_DRAFTS_DIR: &str = "verification/template-drafts";
/// Where rendered artifacts land, unreviewed (S3/S6).
pub const UNREVIEWED_DIR: &str = "verification/unreviewed";
/// The S1 opt-in marker: nothing renders or runs without it.
pub const OPT_IN_FILE: &str = ".phronesis/verification.json";

/// The closed vocabulary of property `kind` values (SPEC-property-ontology.md
/// §2 `property_kind`) — every value this repository's own
/// `.phronesis/properties.json` and specs actually use. Checked at render
/// time (store.rs's ingest-time field-class contract is a separate layer);
/// an unrecognized `kind` is refused before it ever reaches template lookup
/// or render scope. Because the value is drawn from a fixed, host-controlled
/// set rather than free text, it can never carry an injection payload — a
/// closed-vocabulary word cannot inject code — so `kind` is left out of both
/// `validate_body` lists entirely (see `ScopeValues::inert_values`): the
/// bug this guards against is a *false* refusal (a legitimate kind name that
/// also happens to be a real language keyword, e.g. `invariant`, tripping
/// the inert-only rule when it appears live in generated code), not an
/// injection risk.
const PROPERTY_KIND_VALUES: &[&str] = &[
    "precondition",
    "postcondition",
    "invariant",
    "equivalence",
    "determinism",
    "soundness",
    "totality",
];

/// Which directory the template came from. Recorded on every result
/// (`ResultRecord::template_origin`) so draft-derived evidence is visible and
/// never binds as a verification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TemplateOrigin {
    /// `verification/templates/` — the trust anchor.
    Templates,
    /// `verification/template-drafts/` — dev only, never verified evidence.
    TemplateDrafts,
}

impl TemplateOrigin {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Templates => "templates",
            Self::TemplateDrafts => "template_drafts",
        }
    }

    fn dir(self) -> &'static str {
        match self {
            Self::Templates => TEMPLATES_DIR,
            Self::TemplateDrafts => TEMPLATE_DRAFTS_DIR,
        }
    }
}

/// What to render.
#[derive(Debug, Clone, Default)]
pub struct RenderRequest {
    pub property_id: String,
    /// Which encoding's verifier; required only when the property carries
    /// more than one encoding.
    pub verifier: Option<String>,
    /// Consult `verification/template-drafts/` when no trusted template
    /// exists. Dev only: results from a draft never count as verified.
    pub allow_drafts: bool,
}

/// A rendered, validated artifact (not yet written).
#[derive(Debug, Clone, Serialize)]
pub struct RenderedArtifact {
    pub property_id: String,
    /// Content hash of the canonical property record (SHA-256). Any edit to
    /// the record changes it (S2 provenance); reordering a set field does not.
    pub property_revision: String,
    pub language: String,
    pub verifier: String,
    /// Root-relative template path.
    pub template_path: String,
    pub template_sha256: String,
    pub template_origin: TemplateOrigin,
    /// Root-relative artifact path under `verification/unreviewed/`.
    pub artifact_path: String,
    pub artifact_sha256: String,
    #[serde(skip)]
    pub body: String,
}

/// The result of `render_to_disk`.
#[derive(Debug, Clone, Serialize)]
pub struct RenderOutcome {
    #[serde(flatten)]
    pub artifact: RenderedArtifact,
    /// `written`, `unchanged` (identical bytes already on disk), or `dry_run`.
    pub disposition: &'static str,
}

#[derive(Debug, Error)]
pub enum RenderPipelineError {
    #[error(
        "verification is not enabled for this project: {OPT_IN_FILE} is absent (S1 opt-in; a human creates it)"
    )]
    NotOptedIn,
    #[error("property store unusable: {0}")]
    Store(#[from] PropertyStoreError),
    #[error("no property with id {id:?} in .phronesis/properties.json")]
    NoSuchProperty { id: String },
    #[error(
        "property {id} is {status:?}: generation requires accepted, agent_verified, or verified, read from properties.json (S2)"
    )]
    NotAccepted { id: String, status: PropertyStatus },
    #[error("property {id} has no encoding{}", verifier.as_ref().map(|v| format!(" for verifier {v:?}")).unwrap_or_default())]
    NoEncoding {
        id: String,
        verifier: Option<String>,
    },
    #[error("property {id} has several encodings ({verifiers}); pass --verifier to pick one")]
    AmbiguousEncoding { id: String, verifiers: String },
    #[error(
        "property {id} has kind {kind:?}, outside the closed vocabulary ({}) — SPEC-property-ontology.md §2 property_kind",
        PROPERTY_KIND_VALUES.join(", ")
    )]
    UnknownKind { id: String, kind: String },
    #[error(
        "language {language:?} has no render instantiation (host escaping and comment syntax are registered for rust only)"
    )]
    UnsupportedLanguage { language: String },
    #[error(
        "{field} {value:?} cannot name a template file (allowed: lowercase ASCII letters, digits, `_`)"
    )]
    InvalidTemplateKey { field: &'static str, value: String },
    #[error("no template {name} in {TEMPLATES_DIR}/{}", if *drafts_checked { format!(" or {TEMPLATE_DRAFTS_DIR}/") } else { String::new() })]
    TemplateMissing { name: String, drafts_checked: bool },
    #[error(
        "only a draft template exists ({path}); drafts are refused without --allow-drafts, and a human promotes a reviewed draft into {TEMPLATES_DIR}/"
    )]
    DraftRefused { path: String },
    #[error("template {path} resolves outside {dir}/")]
    TemplateEscapes { path: String, dir: &'static str },
    #[error("template {path} failed to render: {message}")]
    Script { path: String, message: String },
    #[error("rendered body refused by S5 validation (nothing written or executed): {0}")]
    Validation(#[from] BodyValidationError),
    #[error("artifact path {path} escapes verification/ (S6)")]
    Containment { path: String },
    #[error(
        "artifact {path} has not been rendered to disk with these bytes; run `phr-mcp verify render` first — an artifact written by this invocation is never executed by it (S3)"
    )]
    NotRendered { path: String },
    #[error(
        "tree revision unavailable: a result must bind to a 40-hex commit (run inside a git checkout with a HEAD)"
    )]
    NoRevision,
    #[error(transparent)]
    Execution(#[from] ExecutionError),
    #[error("journal write failed (nothing committed unaudited): {0}")]
    Journal(#[from] action_log::LogError),
    #[error("{context}: {source}")]
    Io {
        context: String,
        #[source]
        source: std::io::Error,
    },
}

impl RenderPipelineError {
    /// Stable reason code for the journal.
    pub fn reason(&self) -> &'static str {
        match self {
            Self::NotOptedIn => "not_opted_in",
            Self::Store(_) => "store_corrupt",
            Self::NoSuchProperty { .. } => "no_such_property",
            Self::NotAccepted { .. } => "not_accepted",
            Self::NoEncoding { .. } => "no_encoding",
            Self::AmbiguousEncoding { .. } => "ambiguous_encoding",
            Self::UnknownKind { .. } => "unknown_kind",
            Self::UnsupportedLanguage { .. } => "unsupported_language",
            Self::InvalidTemplateKey { .. } => "invalid_template_key",
            Self::TemplateMissing { .. } => "template_missing",
            Self::DraftRefused { .. } => "draft_refused",
            Self::TemplateEscapes { .. } => "template_escapes",
            Self::Script { .. } => "render_failed",
            Self::Validation(_) => "validation_refused",
            Self::Containment { .. } => "containment",
            Self::NotRendered { .. } => "not_rendered",
            Self::NoRevision => "no_revision",
            Self::Execution(_) => "execution_refused",
            Self::Journal(_) => "journal_failed",
            Self::Io { .. } => "io",
        }
    }
}

fn io_err(context: impl Into<String>) -> impl FnOnce(std::io::Error) -> RenderPipelineError {
    let context = context.into();
    move |source| RenderPipelineError::Io { context, source }
}

/// The canonical content hash of a property record: set-valued fields are
/// sorted first, so reordering `depends_on` is not an edit but changing any
/// value is.
pub fn property_revision(p: &Property) -> String {
    let mut canonical = p.clone();
    canonical.depends_on.sort();
    canonical.corroborated_by.sort();
    canonical.encodings.sort_by(|a, b| {
        (&a.language, &a.verifier, &a.artifact).cmp(&(&b.language, &b.verifier, &b.artifact))
    });
    let bytes = serde_json::to_vec(&canonical).unwrap_or_default();
    artifact_sha256(&bytes)
}

/// A template file-name component: `<verifier>-<kind>.rhai`.
fn template_key_ok(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

fn pick_encoding<'a>(
    property: &'a Property,
    verifier: Option<&str>,
) -> Result<&'a Encoding, RenderPipelineError> {
    let candidates: Vec<&Encoding> = property
        .encodings
        .iter()
        .filter(|e| verifier.is_none_or(|v| e.verifier == v))
        .collect();
    match candidates.as_slice() {
        [one] => Ok(one),
        [] => Err(RenderPipelineError::NoEncoding {
            id: property.id.clone(),
            verifier: verifier.map(str::to_string),
        }),
        many => Err(RenderPipelineError::AmbiguousEncoding {
            id: property.id.clone(),
            verifiers: many
                .iter()
                .map(|e| e.verifier.as_str())
                .collect::<Vec<_>>()
                .join(", "),
        }),
    }
}

/// A resolved template: its root-relative path, bytes, and origin.
struct Template {
    rel_path: String,
    source: String,
    origin: TemplateOrigin,
}

/// Load `<dir>/<name>` when it exists; a symlinked template must still
/// resolve inside its directory.
fn load_template(
    root: &Path,
    origin: TemplateOrigin,
    name: &str,
) -> Result<Option<Template>, RenderPipelineError> {
    let rel_path = format!("{}/{name}", origin.dir());
    let path = root.join(&rel_path);
    if !path.is_file() {
        return Ok(None);
    }
    // The trust anchor's directory entry itself must be a real directory,
    // never a symlink (e.g. `verification/templates` -> `template-drafts`):
    // otherwise the starts-with-`dir` containment check below would follow
    // it and pass — the file really does resolve inside `dir`, because `dir`
    // canonicalized to the drafts directory too — silently trusting
    // draft-origin bytes under the `templates` origin. Refusing here makes
    // this look like a missing trusted template, so the caller falls
    // through to the ordinary draft-vs-trusted flow (and the correct origin
    // label) rather than getting a bespoke error.
    if origin == TemplateOrigin::Templates {
        let anchor_is_symlink = std::fs::symlink_metadata(root.join(origin.dir()))
            .is_ok_and(|m| m.file_type().is_symlink());
        if anchor_is_symlink {
            return Ok(None);
        }
    }
    let dir = root
        .join(origin.dir())
        .canonicalize()
        .map_err(io_err(format!("resolve {}", origin.dir())))?;
    let resolved = path
        .canonicalize()
        .map_err(io_err(format!("resolve {rel_path}")))?;
    if !resolved.starts_with(&dir) {
        return Err(RenderPipelineError::TemplateEscapes {
            path: rel_path,
            dir: origin.dir(),
        });
    }
    let source = std::fs::read_to_string(&resolved).map_err(io_err(format!("read {rel_path}")))?;
    Ok(Some(Template {
        rel_path,
        source,
        origin,
    }))
}

/// Look the template up: the trusted directory first; the drafts directory
/// only when `allow_drafts`. A draft that exists while the flag is off is a
/// named refusal, never a silent fallback.
fn resolve_template(
    root: &Path,
    verifier: &str,
    kind: &str,
    allow_drafts: bool,
) -> Result<Template, RenderPipelineError> {
    for (field, value) in [("verifier", verifier), ("kind", kind)] {
        if !template_key_ok(value) {
            return Err(RenderPipelineError::InvalidTemplateKey {
                field,
                value: value.to_string(),
            });
        }
    }
    let name = format!("{verifier}-{kind}.rhai");
    if let Some(t) = load_template(root, TemplateOrigin::Templates, &name)? {
        return Ok(t);
    }
    if allow_drafts {
        if let Some(t) = load_template(root, TemplateOrigin::TemplateDrafts, &name)? {
            return Ok(t);
        }
    } else {
        let draft = format!("{TEMPLATE_DRAFTS_DIR}/{name}");
        if root.join(&draft).is_file() {
            return Err(RenderPipelineError::DraftRefused { path: draft });
        }
    }
    Err(RenderPipelineError::TemplateMissing {
        name,
        drafts_checked: allow_drafts,
    })
}

/// The values the host places in render scope, escaped host-side before
/// they enter Rhai (S5 field-class contract), split into the two lists
/// `validate_body` checks: free text, held to the inert-only rule, and
/// identifier candidates (the subject and fn/type names parsed from
/// `depends_on`), which may additionally appear in live code as a complete
/// identifier or path token. `validate_body` re-checks each identifier
/// candidate for being identifier-shaped itself — a non-identifier-shaped
/// subject (or a `depends_on` entry too irregular to name a function) is
/// still refused in live code, exactly as before this split existed.
struct ScopeValues {
    /// The escaped id, as the `property_depends_on` fact's first argument.
    #[cfg_attr(not(feature = "rhai"), allow(dead_code))]
    id: String,
    fields: Vec<(&'static str, String)>,
    depends_on: Vec<String>,
    /// Subject plus fn/type names parsed from `depends_on` (§ identifier
    /// values above).
    identifiers: Vec<String>,
}

impl ScopeValues {
    fn for_rust(p: &Property) -> Self {
        let esc = |s: &str| escape_rust_string_literal(s);
        let mut depends_on: Vec<String> = p.depends_on.iter().map(|d| esc(d)).collect();
        depends_on.sort();
        depends_on.dedup();
        let id = esc(&p.id);
        let subject = esc(&p.subject);
        let mut identifiers = vec![subject.clone()];
        for region in &p.depends_on {
            identifiers.extend(identifier_names_from_region(region));
        }
        identifiers.sort();
        identifiers.dedup();
        Self {
            fields: vec![
                ("id", id.clone()),
                ("subject", subject),
                ("kind", esc(&p.kind)),
                ("condition", esc(p.condition.as_deref().unwrap_or(""))),
                ("guarantee", esc(p.guarantee.as_deref().unwrap_or(""))),
            ],
            id,
            depends_on,
            identifiers,
        }
    }

    /// Free-text scope values (non-empty ones are checked): every field
    /// except `subject` (an identifier candidate instead, see
    /// `identifier_values`) and `kind` (a closed-vocabulary word —
    /// `PROPERTY_KIND_VALUES`, checked and refused before this is ever built
    /// — so it is not free text at all, and holding it to the inert-only
    /// rule would only produce false refusals when a kind name is also a
    /// real language keyword, e.g. `invariant`), plus the raw `depends_on`
    /// region strings — those remain inert-only even though fn names are
    /// also parsed out of them, since a whole region id
    /// (`fn:src/lib.rs::divide`) is never itself a valid Rust identifier or
    /// path.
    fn inert_values(&self) -> Vec<&str> {
        self.fields
            .iter()
            .filter(|(key, _)| *key != "subject" && *key != "kind")
            .map(|(_, v)| v.as_str())
            .chain(self.depends_on.iter().map(String::as_str))
            .filter(|v| !v.is_empty())
            .collect()
    }

    /// Identifier candidates: the subject and fn/type names parsed from
    /// `depends_on` (non-empty ones are checked).
    fn identifier_values(&self) -> Vec<&str> {
        self.identifiers
            .iter()
            .map(String::as_str)
            .filter(|v| !v.is_empty())
            .collect()
    }
}

/// Parse the fn/type name(s) named by one `depends_on` region id — `fn:` or
/// `branch:` (`crate::coverage::region_map`: `"fn:" file "::" item-path` /
/// `"branch:" file "::" item-path ":" anchor`) — into identifier candidates:
/// the full item path (segments joined by `::`, with a branch entry's
/// trailing `:anchor[.ordinal]` stripped from the last segment) and, on its
/// own, that path's last segment (the fn/type name a template would call).
/// Anything else (a bare/legacy region id with no `file::item-path` shape,
/// or a segment stripped down to empty) yields no candidates — `identifiers`
/// only ever grows the identifier list, never removes a value from the
/// inert-only rule.
fn identifier_names_from_region(region: &str) -> Vec<String> {
    let Some(rest) = region
        .strip_prefix("fn:")
        .or_else(|| region.strip_prefix("branch:"))
    else {
        return Vec::new();
    };
    let Some((_file, item_path)) = rest.split_once("::") else {
        return Vec::new();
    };
    let mut segments: Vec<&str> = item_path.split("::").collect();
    if let Some(last) = segments.last_mut() {
        // A branch entry's last segment is `name:anchor[.ordinal]`; a fn
        // entry's has no such suffix, so `split_once` is a no-op for it.
        if let Some((name, _anchor)) = last.split_once(':') {
            *last = name;
        }
    }
    if segments.iter().any(|s| s.is_empty()) {
        return Vec::new();
    }
    let mut out = vec![segments.join("::")];
    if let Some(last) = segments.last() {
        out.push((*last).to_string());
    }
    out
}

/// Run the template through the dedicated render entry: `property` (a
/// read-only map) and `facts` (frozen, sorted `property_depends_on` facts)
/// are the whole scope; the script returns exactly one string.
#[cfg(feature = "rhai")]
fn run_template(template: &Template, values: &ScopeValues) -> Result<String, RenderPipelineError> {
    use phronesis_rhai::rhai::{Array, Dynamic, Map};
    let mut property = Map::new();
    for (key, value) in &values.fields {
        property.insert((*key).into(), value.as_str().into());
    }
    let depends_on: Array = values
        .depends_on
        .iter()
        .map(|d| Dynamic::from(d.clone()))
        .collect();
    property.insert("depends_on".into(), Dynamic::from(depends_on));
    let facts: Vec<Map> = values
        .depends_on
        .iter()
        .map(|region| {
            let mut fact = Map::new();
            fact.insert("predicate".into(), "property_depends_on".into());
            let args: Array = vec![
                Dynamic::from(values.id.clone()),
                Dynamic::from(region.clone()),
            ];
            fact.insert("args".into(), Dynamic::from(args));
            fact
        })
        .collect();
    phronesis_rhai::render(
        &template.source,
        &phronesis_rhai::RenderInput::frozen(property, facts),
    )
    .map_err(|e| RenderPipelineError::Script {
        path: template.rel_path.clone(),
        message: e.to_string(),
    })
}

#[cfg(not(feature = "rhai"))]
fn run_template(template: &Template, _values: &ScopeValues) -> Result<String, RenderPipelineError> {
    Err(RenderPipelineError::Script {
        path: template.rel_path.clone(),
        message: "this build has no Rhai render engine (built without the `rhai` feature)"
            .to_string(),
    })
}

/// The host-stamped provenance header (outside the render cap, spec
/// "Post-implementation calibration"). Every value in it is host-owned or
/// charset-validated, and it sits in comments, so it is inert under S5.
fn provenance_header(
    property_id: &str,
    revision: &str,
    template: &Template,
    template_sha256: &str,
) -> String {
    format!(
        "// GENERATED — DO NOT EDIT (phr-mcp verify render)\n\
         // property: {property_id} revision: {revision}\n\
         // template: {} sha256: {template_sha256} origin: {}\n",
        template.rel_path,
        template.origin.as_str(),
    )
}

/// The host-generated artifact file name (S6): property slug, verifier, and a
/// short content hash, so two properties never share a file and differing
/// content never reuses a name. Underscores only — the file stem becomes a
/// crate name for rustc-based verifiers.
fn artifact_file_name(property_id: &str, verifier: &str, sha: &str) -> String {
    let slug: String = property_id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    format!("{slug}__{verifier}_{}.rs", &sha[..12])
}

fn opted_in(root: &Path) -> bool {
    root.join(OPT_IN_FILE).is_file()
}

/// Render and validate, writing nothing. The S1 opt-in and the S2 status are
/// read from disk here, every time.
pub fn prepare(root: &Path, req: &RenderRequest) -> Result<RenderedArtifact, RenderPipelineError> {
    if !opted_in(root) {
        return Err(RenderPipelineError::NotOptedIn);
    }
    let property = load_properties(root)?
        .into_iter()
        .find(|p| p.id == req.property_id)
        .ok_or_else(|| RenderPipelineError::NoSuchProperty {
            id: req.property_id.clone(),
        })?;
    if !property.generation_eligible() {
        return Err(RenderPipelineError::NotAccepted {
            id: property.id,
            status: property.status,
        });
    }
    if !PROPERTY_KIND_VALUES.contains(&property.kind.as_str()) {
        return Err(RenderPipelineError::UnknownKind {
            id: property.id,
            kind: property.kind,
        });
    }
    let encoding = pick_encoding(&property, req.verifier.as_deref())?;
    if encoding.language != "rust" {
        return Err(RenderPipelineError::UnsupportedLanguage {
            language: encoding.language.clone(),
        });
    }
    let template = resolve_template(root, &encoding.verifier, &property.kind, req.allow_drafts)?;
    let template_sha256 = artifact_sha256(template.source.as_bytes());
    let values = ScopeValues::for_rust(&property);
    let rendered = run_template(&template, &values)?;
    let revision = property_revision(&property);
    let mut body = provenance_header(&property.id, &revision, &template, &template_sha256);
    body.push_str(&rendered);
    if !body.ends_with('\n') {
        body.push('\n');
    }
    // S5 layer 2: the whole body (header included) must lex, parse, avoid the
    // deny-list, hold every free-text value only in inert text, and hold
    // every identifier candidate either in inert text or as a sanctioned
    // identifier/path token (never a substring, macro name, or sub-path).
    validate_body(
        &encoding.language,
        &body,
        &values.inert_values(),
        &values.identifier_values(),
    )?;
    let sha = artifact_sha256(body.as_bytes());
    let artifact_path = format!(
        "{UNREVIEWED_DIR}/{}",
        artifact_file_name(&property.id, &encoding.verifier, &sha)
    );
    let (language, verifier) = (encoding.language.clone(), encoding.verifier.clone());
    Ok(RenderedArtifact {
        property_id: property.id,
        property_revision: revision,
        language,
        verifier,
        template_path: template.rel_path,
        template_sha256,
        template_origin: template.origin,
        artifact_path,
        artifact_sha256: sha,
        body,
    })
}

fn journal(root: &Path, entry: &LogEntry) -> Result<(), action_log::LogError> {
    action_log::append_audit(&action_log::default_path(root), entry)
}

fn refusal_entry(event: &str, req: &RenderRequest, e: &RenderPipelineError) -> LogEntry {
    LogEntry::new("verification", event)
        .with("property", req.property_id.as_str())
        .with("allow_drafts", req.allow_drafts)
        .with("reason", e.reason())
        .with("error", e.to_string())
}

fn artifact_entry(event: &str, a: &RenderedArtifact) -> LogEntry {
    // S7: bodies journal as hash + path, never inline.
    LogEntry::new("verification", event)
        .with("property", a.property_id.as_str())
        .with("property_revision", a.property_revision.as_str())
        .with("verifier", a.verifier.as_str())
        .with("template", a.template_path.as_str())
        .with("template_sha256", a.template_sha256.as_str())
        .with("template_origin", a.template_origin.as_str())
        .with("artifact", a.artifact_path.as_str())
        .with("artifact_sha256", a.artifact_sha256.as_str())
}

/// Resolve the artifact's absolute path, creating `verification/unreviewed/`
/// when `create`, and refuse any resolution outside `verification/` (S6).
fn contained_artifact_path(
    root: &Path,
    rel: &str,
    create: bool,
) -> Result<PathBuf, RenderPipelineError> {
    let dir = root.join(UNREVIEWED_DIR);
    if create {
        std::fs::create_dir_all(&dir).map_err(io_err(format!("create {UNREVIEWED_DIR}")))?;
    }
    let containment = || RenderPipelineError::Containment {
        path: rel.to_string(),
    };
    let verification = root
        .join("verification")
        .canonicalize()
        .map_err(|_| containment())?;
    let dir = dir.canonicalize().map_err(|_| containment())?;
    if !dir.starts_with(&verification) {
        return Err(containment());
    }
    let file = Path::new(rel).file_name().ok_or_else(containment)?;
    Ok(dir.join(file))
}

/// Render, validate, and (unless `dry_run`) write the artifact into
/// `verification/unreviewed/`. Every generation and every refusal is
/// journaled (S7); a refused body is never written.
pub fn render_to_disk(
    root: &Path,
    req: &RenderRequest,
    dry_run: bool,
) -> Result<RenderOutcome, RenderPipelineError> {
    let artifact = match prepare(root, req) {
        Ok(a) => a,
        Err(e) => {
            // Best effort: the refusal itself is the error the caller sees.
            let _ = journal(root, &refusal_entry("verify_render_refused", req, &e));
            return Err(e);
        }
    };
    if dry_run {
        return Ok(RenderOutcome {
            artifact,
            disposition: "dry_run",
        });
    }
    let path = contained_artifact_path(root, &artifact.artifact_path, true)?;
    if std::fs::read(&path).is_ok_and(|on_disk| on_disk == artifact.body.as_bytes()) {
        journal(
            root,
            &artifact_entry("verify_render", &artifact).with("outcome", "unchanged"),
        )?;
        return Ok(RenderOutcome {
            artifact,
            disposition: "unchanged",
        });
    }
    // Journal before the write: no artifact lands unaudited.
    journal(root, &artifact_entry("verify_render", &artifact))?;
    let staged = path.with_extension("rs.tmp");
    std::fs::write(&staged, artifact.body.as_bytes())
        .map_err(io_err(format!("write {}", artifact.artifact_path)))?;
    std::fs::rename(&staged, &path).map_err(io_err(format!("write {}", artifact.artifact_path)))?;
    Ok(RenderOutcome {
        artifact,
        disposition: "written",
    })
}

/// Execute a previously rendered artifact and record the bound result.
///
/// The artifact is re-rendered in memory (same S1/S2/S5 checks) and must
/// already be on disk with exactly those bytes — written by an earlier
/// `render_to_disk`, never by this call (S3 same-fire prohibition). The
/// executor then re-hashes it, requires a human allowlist approval, and runs
/// it confined (S4/S9). The result is bound to `tree_revision` and carries
/// the template origin; a draft-origin result never hydrates as verified.
pub fn run(
    root: &Path,
    req: &RenderRequest,
    verifier_command: Option<&str>,
    tree_revision: Option<&str>,
) -> Result<ResultRecord, RenderPipelineError> {
    let outcome = run_inner(root, req, verifier_command, tree_revision);
    if let Err(e) = &outcome {
        let _ = journal(root, &refusal_entry("verify_run_refused", req, e));
    }
    outcome
}

fn run_inner(
    root: &Path,
    req: &RenderRequest,
    verifier_command: Option<&str>,
    tree_revision: Option<&str>,
) -> Result<ResultRecord, RenderPipelineError> {
    let artifact = prepare(root, req)?;
    let revision = tree_revision.ok_or(RenderPipelineError::NoRevision)?;
    let not_rendered = || RenderPipelineError::NotRendered {
        path: artifact.artifact_path.clone(),
    };
    let path = contained_artifact_path(root, &artifact.artifact_path, false)
        .map_err(|_| not_rendered())?;
    let on_disk = std::fs::read(&path).map_err(|_| not_rendered())?;
    if on_disk != artifact.body.as_bytes() {
        return Err(not_rendered());
    }
    let command = verifier_command.unwrap_or(&artifact.verifier);
    let mut record = execute::execute(
        root,
        &artifact.property_id,
        &artifact.verifier,
        &path,
        &artifact.artifact_sha256,
        command,
        revision,
    )?;
    record.template_origin = Some(artifact.template_origin.as_str().to_string());
    let tier = record.tier.as_deref().unwrap_or_default();
    journal(
        root,
        &artifact_entry("verify_run", &artifact)
            .with("status", record.status.as_str())
            .with("revision", record.revision.as_str())
            .with("tier", tier),
    )?;
    crate::properties::store::append_result(root, &record)?;
    Ok(record)
}
