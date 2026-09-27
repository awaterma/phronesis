//! `phr-mcp verify` — the CLI over `properties::render`
//! (SPEC-verification-artifact-generation.md).
//!
//! `render` writes a validated artifact into `verification/unreviewed/` for
//! human review; `run` executes a previously rendered, allowlisted artifact
//! and records the bound result. They are separate invocations by design: an
//! artifact is never executed by the invocation that wrote it (S3).

use std::path::Path;

use crate::properties::render::{self, RenderOutcome, RenderRequest, TemplateOrigin};

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
    }
}
