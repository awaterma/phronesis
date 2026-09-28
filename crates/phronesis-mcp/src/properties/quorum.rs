//! Agent-verified evidence (D10; SPEC-verification-artifact-generation.md
//! "Agent-verified evidence"): reviewer records, the quorum rules, and the
//! host-run mechanical checks that gate an `agent_quorum` allowlist entry.
//!
//! Reviewer records live in `.phronesis/verification-reviews.jsonl` — not a
//! trust anchor; agents write it. Reviewer identity (model, family) is
//! self-declared and not authenticated: the quorum rules constrain what is
//! declared, and only the host checks are enforced independently of the
//! reviewers.
//!
//! The host checks run before an approval is admitted, in order:
//! (c) production reach — the artifact's token stream calls a function named
//! by a `fn:`/`branch:` dependency; (b) mutation — a property-declared
//! mutant must make the verifier report `failed`; the baseline artifact must
//! report `passed`; (a) vacuity sentinel — an always-false assertion placed
//! first in each proof site must make the verifier report `failed`. Every
//! run goes through `execute::run_confined` (the S9 tier ladder).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use proc_macro2::{Delimiter, TokenStream, TokenTree};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::action_log::{self, LogEntry};
use crate::properties::allowlist::{
    self, AllowlistEntry, AllowlistError, PrincipalKind, QuorumChecks, QuorumEvidence,
    QuorumReviewer, family_key, quorum_rule_problem,
};
use crate::properties::execute::{self, ConfinedStatus, ExecutionError, artifact_sha256};
use crate::properties::render::{self, RenderPipelineError, RenderRequest, RenderedArtifact};
use crate::properties::store::{Property, load_properties};
use crate::properties::validate::{BodyValidationError, validate_body};

/// Reviewer-record format.
pub const REVIEWS_FORMAT: u32 = 1;

pub fn reviews_path(root: &Path) -> PathBuf {
    root.join(".phronesis").join("verification-reviews.jsonl")
}

/// One reviewer's verdict on one artifact's bytes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReviewRecord {
    pub v: u32,
    pub property_id: String,
    pub artifact_sha256: String,
    pub reviewer_model: String,
    pub reviewer_family: String,
    /// `approve` | `reject`.
    pub verdict: String,
    #[serde(default)]
    pub notes: String,
    pub date: String,
}

#[derive(Debug, Error)]
pub enum QuorumError {
    #[error(transparent)]
    Pipeline(#[from] RenderPipelineError),
    #[error(
        "artifact {path} has not been rendered to disk with these bytes; run `phr-mcp verify render` first"
    )]
    NotRendered { path: String },
    #[error(
        "--artifact-sha256 {given} does not match the current render {actual}: a review names the bytes that exist now"
    )]
    ShaMismatch { given: String, actual: String },
    #[error("review record refused: {message}")]
    InvalidReview { message: String },
    #[error("verification-reviews.jsonl line {line}: {message}")]
    ReviewsCorrupt { line: usize, message: String },
    #[error("quorum refused: {message}")]
    Rules { message: String },
    #[error(
        "production reach refused: the artifact calls none of the depends_on functions ({wanted}) — checked on the token stream, so comments and string literals do not count"
    )]
    NoReach { wanted: String },
    #[error(
        "mutation refused: property {id} declares no mutation for verifier {verifier:?}; a quorum without a mutation check is refused"
    )]
    NoMutation { id: String, verifier: String },
    #[error(
        "mutation refused: find text {find:?} occurs {count} times in the artifact (exactly 1 required)"
    )]
    MutationNotUnique { find: String, count: usize },
    #[error("mutation refused: the mutant fails S5 validation: {0}")]
    MutantInvalid(BodyValidationError),
    #[error(
        "mutation refused: the verifier reported {status} on the mutant replacing {find:?}; it must report failed"
    )]
    MutantSurvived { find: String, status: String },
    #[error(
        "baseline refused: the verifier reported {status} on the unmodified artifact; it must report passed"
    )]
    Baseline { status: String },
    #[error("vacuity sentinel refused: no proof sites found for verifier {verifier:?}")]
    NoSentinelSites { verifier: String },
    #[error(
        "vacuity sentinel refused: the sentinel copy for site {site} fails S5 validation: {source}"
    )]
    SentinelInvalid {
        site: String,
        source: BodyValidationError,
    },
    #[error(
        "vacuity sentinel refused: with an always-false assertion first in {site}, the verifier reported {status}; it must report failed (a passing sentinel means the proof is vacuous)"
    )]
    SentinelPassed { site: String, status: String },
    #[error("verifier {verifier:?} has no vacuity-sentinel instantiation (verus, kani)")]
    UnsupportedVerifier { verifier: String },
    #[error(transparent)]
    Execution(#[from] ExecutionError),
    #[error(transparent)]
    Allowlist(#[from] AllowlistError),
    #[error("journal write failed (nothing committed unaudited): {0}")]
    Journal(#[from] action_log::LogError),
    #[error("{context}: {source}")]
    Io {
        context: String,
        #[source]
        source: std::io::Error,
    },
}

impl QuorumError {
    /// The refusal stage journaled with `verify_quorum_refused`.
    pub fn stage(&self) -> &'static str {
        match self {
            Self::Rules { .. } => "quorum_rules",
            Self::NoReach { .. } => "reach",
            Self::NoMutation { .. }
            | Self::MutationNotUnique { .. }
            | Self::MutantInvalid(_)
            | Self::MutantSurvived { .. } => "mutation",
            Self::Baseline { .. } => "baseline",
            Self::NoSentinelSites { .. }
            | Self::SentinelInvalid { .. }
            | Self::SentinelPassed { .. }
            | Self::UnsupportedVerifier { .. } => "sentinel",
            _ => "input",
        }
    }
}

fn io_err(context: impl Into<String>) -> impl FnOnce(std::io::Error) -> QuorumError {
    let context = context.into();
    move |source| QuorumError::Io { context, source }
}

fn journal(root: &Path, entry: &LogEntry) -> Result<(), action_log::LogError> {
    action_log::append_audit(&action_log::default_path(root), entry)
}

fn today() -> String {
    chrono::Utc::now().format("%Y-%m-%d").to_string()
}

/// Render in memory (S1/S2/S5, exactly as `verify run`) and require those
/// bytes already on disk from an earlier `verify render`.
fn rendered_on_disk(
    root: &Path,
    req: &RenderRequest,
) -> Result<(RenderedArtifact, PathBuf), QuorumError> {
    let artifact = render::prepare(root, req)?;
    let not_rendered = || QuorumError::NotRendered {
        path: artifact.artifact_path.clone(),
    };
    let verification = root
        .join("verification")
        .canonicalize()
        .map_err(|_| not_rendered())?;
    let path = root
        .join(&artifact.artifact_path)
        .canonicalize()
        .map_err(|_| not_rendered())?;
    if !path.starts_with(&verification) {
        return Err(not_rendered());
    }
    let on_disk = std::fs::read(&path).map_err(|_| not_rendered())?;
    if on_disk != artifact.body.as_bytes() {
        return Err(not_rendered());
    }
    Ok((artifact, path))
}

fn review_field_problem(value: &str, field: &str, required: bool) -> Option<String> {
    if required && value.trim().is_empty() {
        return Some(format!("{field} is empty"));
    }
    if value.len() > 512 {
        return Some(format!("{field} exceeds 512 bytes"));
    }
    if value.chars().any(|c| c.is_control()) {
        return Some(format!("{field} carries control characters"));
    }
    None
}

fn review_problem(r: &ReviewRecord) -> Option<String> {
    if r.v != REVIEWS_FORMAT {
        return Some(format!("unsupported format {}", r.v));
    }
    if !matches!(r.verdict.as_str(), "approve" | "reject") {
        return Some(format!("verdict {:?} is not approve or reject", r.verdict));
    }
    if !(r.artifact_sha256.len() == 64
        && r.artifact_sha256
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
    {
        return Some("artifact_sha256 is not 64 lowercase hex".to_string());
    }
    for (value, field, required) in [
        (&r.property_id, "property_id", true),
        (&r.reviewer_model, "reviewer_model", true),
        (&r.reviewer_family, "reviewer_family", true),
        (&r.notes, "notes", false),
        (&r.date, "date", true),
    ] {
        if let Some(problem) = review_field_problem(value, field, required) {
            return Some(problem);
        }
    }
    None
}

/// Load every reviewer record with the SHA-256 of its line. All-or-nothing:
/// a malformed line makes the file unusable, so no quorum is admitted.
pub fn load_reviews(root: &Path) -> Result<Vec<(ReviewRecord, String)>, QuorumError> {
    let path = reviews_path(root);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let raw = std::fs::read_to_string(&path).map_err(io_err("read verification-reviews.jsonl"))?;
    let mut out = Vec::new();
    for (i, line) in raw.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let rec: ReviewRecord =
            serde_json::from_str(line).map_err(|e| QuorumError::ReviewsCorrupt {
                line: i + 1,
                message: e.to_string(),
            })?;
        if let Some(message) = review_problem(&rec) {
            return Err(QuorumError::ReviewsCorrupt {
                line: i + 1,
                message,
            });
        }
        out.push((rec, artifact_sha256(line.as_bytes())));
    }
    Ok(out)
}

/// What a reviewer declares.
#[derive(Debug, Clone)]
pub struct ReviewInput {
    pub artifact_sha256: String,
    pub reviewer_model: String,
    pub reviewer_family: String,
    pub verdict: String,
    pub notes: String,
}

/// `phr-mcp verify review`: record one reviewer's verdict on the current
/// rendered bytes. Refuses unless `artifact_sha256` is the current render's
/// hash and those bytes are on disk.
pub fn record_review(
    root: &Path,
    req: &RenderRequest,
    input: &ReviewInput,
) -> Result<ReviewRecord, QuorumError> {
    let (artifact, _) = rendered_on_disk(root, req)?;
    if input.artifact_sha256 != artifact.artifact_sha256 {
        return Err(QuorumError::ShaMismatch {
            given: input.artifact_sha256.clone(),
            actual: artifact.artifact_sha256,
        });
    }
    let rec = ReviewRecord {
        v: REVIEWS_FORMAT,
        property_id: artifact.property_id.clone(),
        artifact_sha256: artifact.artifact_sha256.clone(),
        reviewer_model: input.reviewer_model.trim().to_string(),
        reviewer_family: input.reviewer_family.trim().to_string(),
        verdict: input.verdict.clone(),
        notes: input.notes.clone(),
        date: today(),
    };
    if let Some(message) = review_problem(&rec) {
        return Err(QuorumError::InvalidReview { message });
    }
    append_review(root, &rec)?;
    journal(
        root,
        &LogEntry::new("verification", "verify_review")
            .with("property", rec.property_id.as_str())
            .with("artifact_sha256", rec.artifact_sha256.as_str())
            .with("reviewer_model", rec.reviewer_model.as_str())
            .with("reviewer_family", rec.reviewer_family.as_str())
            .with("verdict", rec.verdict.as_str()),
    )?;
    Ok(rec)
}

fn append_review(root: &Path, rec: &ReviewRecord) -> Result<(), QuorumError> {
    use fs2::FileExt as _;
    use std::io::Write as _;
    let line = serde_json::to_string(rec).map_err(|e| QuorumError::InvalidReview {
        message: e.to_string(),
    })?;
    let path = reviews_path(root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(io_err("create .phronesis"))?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(io_err("open verification-reviews.jsonl"))?;
    file.lock_exclusive()
        .map_err(io_err("lock verification-reviews.jsonl"))?;
    file.write_all(format!("{line}\n").as_bytes())
        .map_err(io_err("append verification-reviews.jsonl"))
}

/// The approving reviewers for these bytes, after the quorum rules
/// (SPEC C "Quorum rules" 1–4).
pub fn quorum_reviewers(
    reviews: &[(ReviewRecord, String)],
    property_id: &str,
    artifact_sha256: &str,
    author_family: &str,
) -> Result<Vec<QuorumReviewer>, QuorumError> {
    let for_bytes: Vec<&(ReviewRecord, String)> = reviews
        .iter()
        .filter(|(r, _)| r.property_id == property_id && r.artifact_sha256 == artifact_sha256)
        .collect();
    if let Some((r, _)) = for_bytes.iter().find(|(r, _)| r.verdict == "reject") {
        return Err(QuorumError::Rules {
            message: format!(
                "reviewer {:?} ({}) rejected these bytes; one reject vetoes the quorum",
                r.reviewer_model, r.reviewer_family
            ),
        });
    }
    // Rule 4 applies to every record for these bytes, approving or not —
    // all are approving by now (a reject already vetoed).
    let pairs: Vec<(String, String)> = for_bytes
        .iter()
        .map(|(r, _)| (r.reviewer_model.clone(), r.reviewer_family.clone()))
        .collect();
    if let Some(message) = quorum_rule_problem(author_family, &pairs) {
        return Err(QuorumError::Rules { message });
    }
    Ok(for_bytes
        .into_iter()
        .map(|(r, line_sha)| QuorumReviewer {
            model: r.reviewer_model.clone(),
            family: family_key(&r.reviewer_family),
            verdict: r.verdict.clone(),
            record_sha256: line_sha.clone(),
        })
        .collect())
}

// ---- token-level structure (proc-macro2: comments are not tokens, string
// literals are opaque Literal tokens) ----

fn lex(body: &str) -> Result<TokenStream, QuorumError> {
    body.parse::<TokenStream>().map_err(|e| {
        QuorumError::Pipeline(RenderPipelineError::Validation(
            BodyValidationError::Unparseable {
                language: "rust".to_string(),
                message: e.to_string(),
            },
        ))
    })
}

fn ident_is(tt: Option<&TokenTree>, name: &str) -> bool {
    matches!(tt, Some(TokenTree::Ident(i)) if i == name)
}

/// Function leaf names a property's `fn:` / `branch:` dependencies name.
pub fn dependency_functions(property: &Property) -> BTreeSet<String> {
    property
        .depends_on
        .iter()
        .filter_map(|d| crate::coverage::region_map::region_function_leaf(d))
        .map(str::to_string)
        .collect()
}

/// The dependency functions the artifact structurally **calls**: the leaf
/// name followed by a parenthesized group, not preceded by `fn` (a
/// definition) and not a macro (`name!(…)` has a `!` between).
pub fn production_reach(body: &str, wanted: &BTreeSet<String>) -> Result<Vec<String>, QuorumError> {
    let tokens = lex(body)?;
    let mut found = BTreeSet::new();
    collect_calls(tokens, wanted, &mut found);
    proc_macro2::extra::invalidate_current_thread_spans();
    Ok(found.into_iter().collect())
}

fn collect_calls(stream: TokenStream, wanted: &BTreeSet<String>, found: &mut BTreeSet<String>) {
    let tokens: Vec<TokenTree> = stream.into_iter().collect();
    for (i, tt) in tokens.iter().enumerate() {
        match tt {
            TokenTree::Group(g) => collect_calls(g.stream(), wanted, found),
            TokenTree::Ident(ident) => {
                let name = ident.to_string();
                if !wanted.contains(&name) {
                    continue;
                }
                let called = matches!(
                    tokens.get(i + 1),
                    Some(TokenTree::Group(g)) if g.delimiter() == Delimiter::Parenthesis
                );
                let defined = i > 0 && ident_is(tokens.get(i - 1), "fn");
                if called && !defined {
                    found.insert(name);
                }
            }
            _ => {}
        }
    }
}

/// A vacuity-sentinel insertion point: just after a proof site's opening
/// body brace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SentinelSite {
    /// The function's name.
    pub name: String,
    /// Byte offset just past the body's `{`.
    pub offset: usize,
}

/// Header modifiers that may precede `fn` in an item.
const FN_MODIFIERS: &[&str] = &[
    "pub",
    "open",
    "closed",
    "spec",
    "proof",
    "exec",
    "broadcast",
    "const",
    "unsafe",
    "async",
    "extern",
    "default",
    "tracked",
    "uninterp",
    "axiom",
];

/// Tokens that start the next item: the brace group before one of these
/// (or at the end of the block) closes a function body.
const ITEM_STARTS: &[&str] = &[
    "fn",
    "pub",
    "open",
    "closed",
    "spec",
    "proof",
    "exec",
    "broadcast",
    "struct",
    "enum",
    "union",
    "impl",
    "trait",
    "mod",
    "use",
    "const",
    "static",
    "type",
    "global",
    "layout",
    "tracked",
    "uninterp",
    "axiom",
    "unsafe",
    "async",
    "extern",
    "macro_rules",
    "verus",
];

fn starts_item(tt: Option<&TokenTree>) -> bool {
    match tt {
        None => true,
        Some(TokenTree::Ident(i)) => ITEM_STARTS.contains(&i.to_string().as_str()),
        Some(TokenTree::Punct(p)) => p.as_char() == '#',
        _ => false,
    }
}

/// Is the `fn` at `i` a `spec` function (header modifiers scanned back)?
fn is_spec_fn(tokens: &[TokenTree], i: usize) -> bool {
    let mut j = i;
    while j > 0 {
        j -= 1;
        match &tokens[j] {
            TokenTree::Ident(id) => {
                let s = id.to_string();
                if s == "spec" {
                    return true;
                }
                if !FN_MODIFIERS.contains(&s.as_str()) {
                    return false;
                }
            }
            // `pub(crate)`, `spec(checked)`.
            TokenTree::Group(g) if g.delimiter() == Delimiter::Parenthesis => {}
            // `extern "C"`.
            TokenTree::Literal(_) => {}
            _ => return false,
        }
    }
    false
}

/// Is the `fn` at `i` preceded by a `#[kani::proof]` attribute?
fn has_kani_proof_attr(tokens: &[TokenTree], i: usize) -> bool {
    let mut j = i;
    // Walk back over modifiers, then over attributes.
    while j > 0
        && matches!(&tokens[j - 1], TokenTree::Ident(id) if FN_MODIFIERS.contains(&id.to_string().as_str()))
    {
        j -= 1;
    }
    while j >= 2 {
        let (TokenTree::Punct(hash), TokenTree::Group(g)) = (&tokens[j - 2], &tokens[j - 1]) else {
            return false;
        };
        if hash.as_char() != '#' || g.delimiter() != Delimiter::Bracket {
            return false;
        }
        let path: String = g
            .stream()
            .into_iter()
            .map(|t| t.to_string())
            .collect::<Vec<_>>()
            .join("");
        if path == "kani::proof" {
            return true;
        }
        j -= 2;
    }
    false
}

/// The body brace group of the `fn` at `i`: the first brace group at this
/// level that is followed by the end of the block or the start of the next
/// item. `None` for a body-less declaration (`;` first).
fn fn_body_offset(tokens: &[TokenTree], i: usize) -> Option<usize> {
    for (k, tt) in tokens.iter().enumerate().skip(i + 1) {
        match tt {
            TokenTree::Punct(p) if p.as_char() == ';' => return None,
            TokenTree::Group(g)
                if g.delimiter() == Delimiter::Brace && starts_item(tokens.get(k + 1)) =>
            {
                return Some(g.span_open().byte_range().end);
            }
            _ => {}
        }
    }
    None
}

fn collect_sites(
    stream: TokenStream,
    wants: &dyn Fn(&[TokenTree], usize) -> bool,
    sites: &mut Vec<SentinelSite>,
) {
    let tokens: Vec<TokenTree> = stream.into_iter().collect();
    for (i, tt) in tokens.iter().enumerate() {
        match tt {
            TokenTree::Group(g) => collect_sites(g.stream(), wants, sites),
            TokenTree::Ident(id) if id == "fn" && wants(&tokens, i) => {
                let name = match tokens.get(i + 1) {
                    Some(TokenTree::Ident(n)) => n.to_string(),
                    _ => continue,
                };
                if let Some(offset) = fn_body_offset(&tokens, i) {
                    sites.push(SentinelSite { name, offset });
                }
            }
            _ => {}
        }
    }
}

/// The always-false assertion for a verifier.
pub fn sentinel_statement(verifier: &str) -> Option<&'static str> {
    match verifier {
        "verus" => Some(" assert(false); "),
        "kani" => Some(" assert!(false); "),
        _ => None,
    }
}

/// Proof sites for the vacuity sentinel. verus: every non-`spec` `fn` with a
/// body inside a `verus! { … }` block (fns outside it are plain Rust, where
/// `assert(false)` does not compile). kani: every `#[kani::proof]` fn.
pub fn sentinel_sites(verifier: &str, body: &str) -> Result<Vec<SentinelSite>, QuorumError> {
    let tokens: Vec<TokenTree> = lex(body)?.into_iter().collect();
    let mut sites = Vec::new();
    match verifier {
        "verus" => {
            for (i, tt) in tokens.iter().enumerate() {
                let is_verus_block = matches!(tt, TokenTree::Ident(id) if id == "verus")
                    && matches!(tokens.get(i + 1), Some(TokenTree::Punct(p)) if p.as_char() == '!');
                if !is_verus_block {
                    continue;
                }
                if let Some(TokenTree::Group(g)) = tokens.get(i + 2)
                    && g.delimiter() == Delimiter::Brace
                {
                    collect_sites(g.stream(), &|t, i| !is_spec_fn(t, i), &mut sites);
                }
            }
        }
        "kani" => {
            collect_sites(
                tokens.into_iter().collect(),
                &has_kani_proof_attr,
                &mut sites,
            );
        }
        _ => {
            proc_macro2::extra::invalidate_current_thread_spans();
            return Err(QuorumError::UnsupportedVerifier {
                verifier: verifier.to_string(),
            });
        }
    }
    proc_macro2::extra::invalidate_current_thread_spans();
    Ok(sites)
}

/// A copy of `body` with the sentinel inserted at `site`.
pub fn sentinel_copy(body: &str, site: &SentinelSite, statement: &str) -> String {
    let mut out = String::with_capacity(body.len() + statement.len());
    out.push_str(&body[..site.offset]);
    out.push_str(statement);
    out.push_str(&body[site.offset..]);
    out
}

/// Apply a mutation: `find` must occur exactly once.
pub fn apply_mutation(body: &str, find: &str, replace: &str) -> Result<String, QuorumError> {
    let count = body.matches(find).count();
    if count != 1 {
        return Err(QuorumError::MutationNotUnique {
            find: find.to_string(),
            count,
        });
    }
    Ok(body.replacen(find, replace, 1))
}

/// The outcome of an admitted quorum approval.
#[derive(Debug, Clone, Serialize)]
pub struct QuorumApproval {
    pub entry: AllowlistEntry,
    /// `recorded`, or `already_approved` when an entry already approves
    /// these bytes for this property (no checks run, nothing written).
    pub disposition: &'static str,
}

/// `phr-mcp verify approve <id> --quorum`: admit an agent-quorum approval
/// only after the quorum rules and every host check pass. Every refusal is
/// journaled with its stage; an admission is journaled before the
/// allowlist write.
pub fn approve_quorum(
    root: &Path,
    req: &RenderRequest,
    author_family: &str,
    verifier_command: Option<&str>,
) -> Result<QuorumApproval, QuorumError> {
    let out = approve_inner(root, req, author_family, verifier_command);
    if let Err(e) = &out {
        let _ = journal(
            root,
            &LogEntry::new("verification", "verify_quorum_refused")
                .with("property", req.property_id.as_str())
                .with("stage", e.stage())
                .with("error", e.to_string()),
        );
    }
    out
}

fn run_status(
    root: &Path,
    bytes: &str,
    file_name: &Path,
    command: &str,
) -> Result<(String, &'static str), QuorumError> {
    let run = execute::run_confined(root, bytes.as_bytes(), file_name, command)?;
    let status = match run.status {
        ConfinedStatus::Parsed(s) => s,
        other => other.as_str().to_string(),
    };
    Ok((status, run.tier.as_str()))
}

fn approve_inner(
    root: &Path,
    req: &RenderRequest,
    author_family: &str,
    verifier_command: Option<&str>,
) -> Result<QuorumApproval, QuorumError> {
    let (artifact, path) = rendered_on_disk(root, req)?;
    let existing = allowlist::load(root)?;
    if let Some(entry) = existing.entries.iter().find(|e| {
        e.artifact_sha256 == artifact.artifact_sha256 && e.property_id == artifact.property_id
    }) {
        return Ok(QuorumApproval {
            entry: entry.clone(),
            disposition: "already_approved",
        });
    }

    // Quorum rules (reviewer records for exactly these bytes).
    let reviews = load_reviews(root)?;
    let reviewers = quorum_reviewers(
        &reviews,
        &artifact.property_id,
        &artifact.artifact_sha256,
        author_family,
    )?;

    let property = load_properties(root)
        .map_err(RenderPipelineError::Store)?
        .into_iter()
        .find(|p| p.id == artifact.property_id)
        .ok_or_else(|| RenderPipelineError::NoSuchProperty {
            id: artifact.property_id.clone(),
        })?;
    let body = artifact.body.as_str();
    let file_name =
        path.file_name()
            .map(PathBuf::from)
            .ok_or_else(|| QuorumError::NotRendered {
                path: artifact.artifact_path.clone(),
            })?;
    let command = verifier_command.unwrap_or(&artifact.verifier);
    let verifier = artifact.verifier.as_str();

    // (c) Production reach — structural, no execution.
    let wanted = dependency_functions(&property);
    let reach = production_reach(body, &wanted)?;
    if reach.is_empty() {
        return Err(QuorumError::NoReach {
            wanted: wanted.into_iter().collect::<Vec<_>>().join(", "),
        });
    }

    // Sentinel sites are found (and the verifier supported) before any run.
    let statement =
        sentinel_statement(verifier).ok_or_else(|| QuorumError::UnsupportedVerifier {
            verifier: verifier.to_string(),
        })?;
    let sites = sentinel_sites(verifier, body)?;
    if sites.is_empty() {
        return Err(QuorumError::NoSentinelSites {
            verifier: verifier.to_string(),
        });
    }
    let mut sentinels = Vec::new();
    for site in &sites {
        let copy = sentinel_copy(body, site, statement);
        validate_body(&artifact.language, &copy, &[], &[]).map_err(|source| {
            QuorumError::SentinelInvalid {
                site: site.name.clone(),
                source,
            }
        })?;
        sentinels.push((site.name.clone(), copy));
    }

    // (b) Mutation — declared on the record; the mutant must fail.
    let mutations: Vec<_> = property
        .mutations
        .iter()
        .filter(|m| m.verifier == verifier)
        .collect();
    if mutations.is_empty() {
        return Err(QuorumError::NoMutation {
            id: property.id.clone(),
            verifier: verifier.to_string(),
        });
    }
    let mut mutants = Vec::new();
    for m in &mutations {
        let mutant = apply_mutation(body, &m.find, &m.replace)?;
        validate_body(&artifact.language, &mutant, &[], &[]).map_err(QuorumError::MutantInvalid)?;
        mutants.push((m.find.clone(), mutant));
    }
    let mut tier = "";
    for (find, mutant) in &mutants {
        let (status, t) = run_status(root, mutant, &file_name, command)?;
        tier = t;
        if status != "failed" {
            return Err(QuorumError::MutantSurvived {
                find: find.clone(),
                status,
            });
        }
    }

    // Baseline — the unmodified artifact must prove.
    let (baseline, _) = run_status(root, body, &file_name, command)?;
    if baseline != "passed" {
        return Err(QuorumError::Baseline { status: baseline });
    }

    // (a) Vacuity sentinel — every site's copy must fail.
    for (site, copy) in &sentinels {
        let (status, _) = run_status(root, copy, &file_name, command)?;
        if status != "failed" {
            return Err(QuorumError::SentinelPassed {
                site: site.clone(),
                status,
            });
        }
    }

    let families: BTreeSet<&str> = reviewers.iter().map(|r| r.family.as_str()).collect();
    let entry = AllowlistEntry {
        artifact_sha256: artifact.artifact_sha256.clone(),
        template_sha256: artifact.template_sha256.clone(),
        property_id: artifact.property_id.clone(),
        property_revision: artifact.property_revision.clone(),
        approver_principal: format!(
            "agent_quorum:{}",
            families.into_iter().collect::<Vec<_>>().join("+")
        ),
        date: today(),
        principal_kind: PrincipalKind::AgentQuorum,
        quorum: Some(QuorumEvidence {
            author_family: family_key(author_family),
            reviewers,
            checks: QuorumChecks {
                reach,
                mutation: "failed".to_string(),
                baseline: "passed".to_string(),
                sentinel_sites: sites.len(),
                tier: tier.to_string(),
            },
        }),
    };
    journal(
        root,
        &LogEntry::new("verification", "verify_quorum_approve")
            .with("property", entry.property_id.as_str())
            .with("artifact_sha256", entry.artifact_sha256.as_str())
            .with("template_sha256", entry.template_sha256.as_str())
            .with("approver_principal", entry.approver_principal.as_str())
            .with("principal_kind", PrincipalKind::AgentQuorum.as_str())
            .with("sentinel_sites", sites.len())
            .with("tier", tier),
    )?;
    allowlist::record(root, entry.clone())?;
    Ok(QuorumApproval {
        entry,
        disposition: "recorded",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const HARNESS: &str = r#"// check: divide(1, 0) in a comment is not a call
use vstd::prelude::*;
verus! {
pub open spec fn claim(d: u32) -> bool { d == 0 }
pub fn divide(n: u32, d: u32) -> (r: u32)
    requires d != 0,
    ensures r == n / d,
{
    n / d
}
proof fn lemma() ensures true, {}
fn check() {
    let s = "divide(2, 1)";
    let r = divide(4, 2);
}
fn main() {
}
}
"#;

    fn wanted(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn reach_counts_calls_not_definitions_comments_or_strings() {
        assert_eq!(
            production_reach(HARNESS, &wanted(&["divide"])).unwrap(),
            vec!["divide".to_string()]
        );
        let no_call = HARNESS.replace("let r = divide(4, 2);", "");
        assert!(
            production_reach(&no_call, &wanted(&["divide"]))
                .unwrap()
                .is_empty(),
            "a definition, a comment and a string literal are not calls"
        );
        assert!(
            production_reach(HARNESS, &wanted(&["claim_other"]))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn verus_sentinel_sites_skip_spec_fns_and_land_inside_bodies() {
        let sites = sentinel_sites("verus", HARNESS).unwrap();
        let names: Vec<&str> = sites.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["divide", "lemma", "check", "main"]);
        let copy = sentinel_copy(HARNESS, &sites[0], " assert(false); ");
        assert!(
            copy.contains("{ assert(false); \n    n / d"),
            "sentinel lands first in divide's body:\n{copy}"
        );
    }

    #[test]
    fn kani_sentinel_sites_are_proof_harnesses_only() {
        let body = "fn helper() -> u8 { 1 }\n#[kani::proof]\nfn check_helper() { assert!(helper() == 1); }\n";
        let sites = sentinel_sites("kani", body).unwrap();
        assert_eq!(sites.len(), 1);
        assert_eq!(sites[0].name, "check_helper");
        assert!(
            sentinel_copy(body, &sites[0], " assert!(false); ")
                .contains("{ assert!(false);  assert!(helper()")
        );
    }

    #[test]
    fn mutation_must_match_exactly_once() {
        assert!(apply_mutation("a b a", "a", "c").is_err());
        assert!(apply_mutation("a b", "z", "c").is_err());
        assert_eq!(apply_mutation("a b", "b", "c").unwrap(), "a c");
    }

    #[test]
    fn quorum_rules_refuse_single_family_author_family_single_reviewer_and_reject() {
        let rec = |model: &str, family: &str, verdict: &str| {
            (
                ReviewRecord {
                    v: 1,
                    property_id: "p".into(),
                    artifact_sha256: "a".repeat(64),
                    reviewer_model: model.into(),
                    reviewer_family: family.into(),
                    verdict: verdict.into(),
                    notes: String::new(),
                    date: "2026-09-27".into(),
                },
                "x".repeat(64),
            )
        };
        let sha = "a".repeat(64);
        let ok = vec![rec("m1", "fam-a", "approve"), rec("m2", "fam-b", "approve")];
        assert!(quorum_reviewers(&ok, "p", &sha, "fam-c").is_ok());
        let one_family = vec![
            rec("m1", "fam-a", "approve"),
            rec("m2", "FAM-A ", "approve"),
        ];
        assert!(quorum_reviewers(&one_family, "p", &sha, "fam-c").is_err());
        assert!(quorum_reviewers(&ok, "p", &sha, "Fam-B").is_err());
        assert!(quorum_reviewers(&ok[..1], "p", &sha, "fam-c").is_err());
        let mut vetoed = ok.clone();
        vetoed.push(rec("m3", "fam-d", "reject"));
        assert!(quorum_reviewers(&vetoed, "p", &sha, "fam-c").is_err());
        // Records for other bytes do not count.
        assert!(quorum_reviewers(&ok, "p", &"b".repeat(64), "fam-c").is_err());
    }
}
