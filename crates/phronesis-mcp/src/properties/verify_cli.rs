//! `phr-mcp verify` — the CLI over `properties::render`
//! (SPEC-verification-artifact-generation.md).
//!
//! `render` writes a validated artifact into `verification/unreviewed/` for
//! human review; `run` executes a previously rendered, allowlisted artifact
//! and records the bound result. They are separate invocations by design: an
//! artifact is never executed by the invocation that wrote it (S3).
//!
//! D10 agent-verified evidence: `review` records one reviewer's verdict on
//! the rendered bytes (`.phronesis/verification-reviews.jsonl`, agent-
//! writable); `approve --quorum` admits an `agent_quorum` allowlist entry
//! only after the quorum rules and the host checks pass (`properties::quorum`).

use std::path::Path;

use crate::properties::quorum::{self, ReviewInput};
use crate::properties::render::{self, RenderOutcome, RenderRequest, TemplateOrigin};

/// A reviewer's verdict.
#[derive(clap::ValueEnum, Clone, Copy, Debug)]
pub enum Verdict {
    Approve,
    Reject,
}

impl Verdict {
    fn as_str(self) -> &'static str {
        match self {
            Self::Approve => "approve",
            Self::Reject => "reject",
        }
    }
}

#[derive(clap::Subcommand, Debug)]
pub enum VerifyCmd {
    /// Render a property's verification artifact into verification/unreviewed/.
    ///
    /// Requires the opt-in `.phronesis/verification.json` and an `accepted` or
    /// `verified` property. The template is `verification/templates/
    /// <verifier>-<kind>.rhai`; the body is validated (S5) before anything is
    /// written. A human then reviews the artifact and records its hash in
    /// `.phronesis/verification-allowlist.json`.
    Render {
        /// The property id from `.phronesis/properties.json`.
        property_id: String,
        /// The encoding's verifier, when the property has several encodings.
        #[arg(long)]
        verifier: Option<String>,
        /// Dev only: fall back to verification/template-drafts/ when no
        /// trusted template exists. Results from a draft never count as
        /// verified evidence.
        #[arg(long)]
        allow_drafts: bool,
        /// Render and validate, print the body, write nothing.
        #[arg(long)]
        dry_run: bool,
        /// Emit one JSON object.
        #[arg(long)]
        json: bool,
    },
    /// Execute a previously rendered, human-approved artifact and record the
    /// bound result in `.phronesis/property-results.jsonl`.
    ///
    /// Re-renders in memory and requires those exact bytes already on disk
    /// (from an earlier `verify render`) and allowlisted, then runs the
    /// verifier confined (devcontainer → sandbox-exec → raw only when
    /// configured → refused), bound to the current HEAD.
    Run {
        /// The property id from `.phronesis/properties.json`.
        property_id: String,
        /// The encoding's verifier, when the property has several encodings.
        #[arg(long)]
        verifier: Option<String>,
        /// Dev only: fall back to verification/template-drafts/. The result is
        /// recorded with `template_origin: template_drafts` and never
        /// hydrates as `verification_result`.
        #[arg(long)]
        allow_drafts: bool,
        /// Verifier program (and fixed leading arguments) to run; the
        /// artifact path is appended by the host. Defaults to the encoding's
        /// verifier name (e.g. `verus`).
        #[arg(long, value_name = "CMD")]
        verifier_command: Option<String>,
        /// Emit the result record as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Record one reviewer's verdict on a rendered artifact (D10 agent
    /// quorum) in `.phronesis/verification-reviews.jsonl`.
    ///
    /// Reviewer model and family are self-declared, not authenticated.
    /// `--artifact-sha256` must equal the current render's hash, and those
    /// bytes must already be on disk (from `verify render`).
    Review {
        /// The property id from `.phronesis/properties.json`.
        property_id: String,
        /// The encoding's verifier, when the property has several encodings.
        #[arg(long)]
        verifier: Option<String>,
        /// SHA-256 of the artifact bytes the reviewer read.
        #[arg(long, value_name = "SHA256")]
        artifact_sha256: String,
        /// The reviewing model (self-declared).
        #[arg(long, value_name = "MODEL")]
        reviewer_model: String,
        /// The reviewing model's family (self-declared); compared trimmed
        /// and lowercased.
        #[arg(long, value_name = "FAMILY")]
        reviewer_family: String,
        #[arg(long, value_enum)]
        verdict: Verdict,
        #[arg(long, default_value = "")]
        notes: String,
        /// Emit the review record as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Admit an agent-quorum approval (D10) into the allowlist, recorded with
    /// principal kind `agent_quorum`; its results hydrate as
    /// `agent_verification_result`, never as human `verification_result`.
    ///
    /// Requires `--quorum`: human approvals are recorded by a human, never
    /// by this command. Admitted only when the reviewer records for the
    /// current bytes meet the quorum rules (≥2 approving, ≥2 families, none
    /// sharing `--author-family`, no reject) and the host checks pass:
    /// production reach, a failing mutant, a passing baseline, and a failing
    /// vacuity sentinel at every proof site — all run confined.
    Approve {
        /// The property id from `.phronesis/properties.json`.
        property_id: String,
        /// The encoding's verifier, when the property has several encodings.
        #[arg(long)]
        verifier: Option<String>,
        /// Approve through the agent review quorum (required).
        #[arg(long)]
        quorum: bool,
        /// The model family that authored the template/property (self-
        /// declared); no reviewer may share it.
        #[arg(long, value_name = "FAMILY")]
        author_family: Option<String>,
        /// Verifier program (and fixed leading arguments) for the checks;
        /// defaults to the encoding's verifier name.
        #[arg(long, value_name = "CMD")]
        verifier_command: Option<String>,
        /// Emit the allowlist entry as JSON.
        #[arg(long)]
        json: bool,
    },
}

fn origin_note(origin: TemplateOrigin) -> &'static str {
    match origin {
        TemplateOrigin::Templates => "",
        TemplateOrigin::TemplateDrafts => {
            " (DRAFT — results from it never count as verified evidence)"
        }
    }
}

fn render_text(out: &RenderOutcome) -> String {
    let a = &out.artifact;
    let disposition = match out.disposition {
        "written" => "written",
        "unchanged" => "unchanged (identical bytes already on disk)",
        _ => "dry run: not written",
    };
    let mut text = format!(
        "rendered {} ({}, {})\n  template:          {}{}\n  template sha256:   {}\n  property revision: {}\n  artifact:          {} — {disposition}\n  artifact sha256:   {}\n",
        a.property_id,
        a.language,
        a.verifier,
        a.template_path,
        origin_note(a.template_origin),
        a.template_sha256,
        a.property_revision,
        a.artifact_path,
        a.artifact_sha256,
    );
    if out.disposition == "dry_run" {
        text.push('\n');
        text.push_str(&a.body);
    } else {
        text.push_str(&format!(
            "next: a human reviews {} and records artifact_sha256, template_sha256, property_id and property_revision in .phronesis/verification-allowlist.json; then `phr-mcp verify run {}{}`",
            a.artifact_path,
            a.property_id,
            if a.template_origin == TemplateOrigin::TemplateDrafts {
                " --allow-drafts"
            } else {
                ""
            },
        ));
    }
    text
}

/// Run one `verify` subcommand against `root`; returns the text to print.
pub fn run(root: &Path, cmd: VerifyCmd) -> anyhow::Result<String> {
    match cmd {
        VerifyCmd::Render {
            property_id,
            verifier,
            allow_drafts,
            dry_run,
            json,
        } => {
            let req = RenderRequest {
                property_id,
                verifier,
                allow_drafts,
            };
            let out = render::render_to_disk(root, &req, dry_run)?;
            if json {
                let mut value = serde_json::to_value(&out)?;
                if dry_run && let Some(obj) = value.as_object_mut() {
                    obj.insert("body".into(), out.artifact.body.clone().into());
                }
                return Ok(serde_json::to_string_pretty(&value)?);
            }
            Ok(render_text(&out))
        }
        VerifyCmd::Run {
            property_id,
            verifier,
            allow_drafts,
            verifier_command,
            json,
        } => {
            let req = RenderRequest {
                property_id,
                verifier,
                allow_drafts,
            };
            let head = crate::lifecycle::outcome::git_head(root);
            let record = render::run(root, &req, verifier_command.as_deref(), head.as_deref())?;
            if json {
                return Ok(serde_json::to_string_pretty(&record)?);
            }
            let draft = record.template_origin.as_deref() == Some("template_drafts");
            Ok(format!(
                "{} {} via {}: {} (tier {}, revision {}){}",
                record.property,
                record.verifier,
                record.tool,
                record.status,
                record.tier.as_deref().unwrap_or("?"),
                record.revision,
                if draft {
                    "\n  draft template: recorded, never counted as verified evidence"
                } else {
                    ""
                },
            ))
        }
        VerifyCmd::Review {
            property_id,
            verifier,
            artifact_sha256,
            reviewer_model,
            reviewer_family,
            verdict,
            notes,
            json,
        } => {
            let req = RenderRequest {
                property_id,
                verifier,
                allow_drafts: false,
            };
            let rec = quorum::record_review(
                root,
                &req,
                &ReviewInput {
                    artifact_sha256,
                    reviewer_model,
                    reviewer_family,
                    verdict: verdict.as_str().to_string(),
                    notes,
                },
            )?;
            if json {
                return Ok(serde_json::to_string_pretty(&rec)?);
            }
            Ok(format!(
                "recorded review of {} ({}): {} by {} [{}] — identity is self-declared, not authenticated",
                rec.property_id,
                rec.artifact_sha256,
                rec.verdict,
                rec.reviewer_model,
                rec.reviewer_family,
            ))
        }
        VerifyCmd::Approve {
            property_id,
            verifier,
            quorum: through_quorum,
            author_family,
            verifier_command,
            json,
        } => {
            if !through_quorum {
                anyhow::bail!(
                    "verify approve records only agent-quorum approvals (pass --quorum); a human approval is recorded by the human in .phronesis/verification-allowlist.json"
                );
            }
            let Some(author_family) = author_family else {
                anyhow::bail!(
                    "--author-family is required with --quorum: no reviewer may share the author's model family"
                );
            };
            let req = RenderRequest {
                property_id,
                verifier,
                allow_drafts: false,
            };
            let out =
                quorum::approve_quorum(root, &req, &author_family, verifier_command.as_deref())?;
            if json {
                return Ok(serde_json::to_string_pretty(&out)?);
            }
            let e = &out.entry;
            Ok(format!(
                "{} {} ({}): principal {} [{}]{}",
                e.property_id,
                out.disposition,
                e.artifact_sha256,
                e.approver_principal,
                e.principal_kind.as_str(),
                if out.disposition == "recorded" {
                    "\n  results for these bytes hydrate as agent_verification_result; the property may move to agent_verified, never verified"
                } else {
                    ""
                },
            ))
        }
    }
}
