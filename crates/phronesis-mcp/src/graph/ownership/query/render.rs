//! Human-readable and JSON rendering of an [`OwnershipReport`].

use std::fmt::Write as _;

use crate::graph::ownership::ANALYSIS_CAPABILITIES;

use super::{
    AnalysisStatus, EvidenceRef, FunctionEvidence, OwnershipReport, OwnershipState, SiteEvidence,
    SourceLocation,
};

/// Longest operand/place/guard text interpolated into a table row. The graph
/// caps the stored value at 240 bytes (D7), which is still far too wide for a
/// terminal column.
const TABLE_TEXT_WIDTH: usize = 72;

/// Collapse every run of whitespace to a single space.
///
/// Operand text is capped at 240 bytes but not at one line, and a multi-line
/// expression interpolated into a table row splits it mid-value and breaks
/// alignment for everything after it. Applied to every interpolated value,
/// not only the ones known to wrap today.
fn one_line(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut pending_space = false;
    for ch in text.chars() {
        if ch.is_whitespace() {
            pending_space = !out.is_empty();
        } else {
            if pending_space {
                out.push(' ');
                pending_space = false;
            }
            out.push(ch);
        }
    }
    out
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let kept: String = text.chars().take(max.saturating_sub(1)).collect();
    format!("{kept}…")
}

fn cell(text: &str) -> String {
    truncate(&one_line(text), TABLE_TEXT_WIDTH)
}

fn kind_phrase(kind: &str) -> &'static str {
    match kind {
        "clone" => "clone",
        "filter" => "filter",
        "await" => "await",
        "mutation" => "mutation",
        "sync_lock" => "sync lock acquired",
        _ => "ownership site",
    }
}

fn operand_label(kind: &str) -> &'static str {
    match kind {
        "mutation" => "place",
        "sync_lock" => "guard",
        _ => "operand",
    }
}

fn location_text(location: Option<&SourceLocation>) -> String {
    match location {
        Some(location) => match location.line {
            Some(line) => format!(
                "{}:{line} (bytes {}..{})",
                location.file, location.start_byte, location.end_byte
            ),
            None => format!(
                "{} bytes {}..{}",
                location.file, location.start_byte, location.end_byte
            ),
        },
        None => "location not indexed".to_string(),
    }
}

/// One-line human summary of a site: what was observed, and where.
pub(super) fn describe_site(site: &SiteEvidence) -> String {
    let phrase = kind_phrase(&site.kind);
    let mut text = phrase.to_string();
    // Name the operation only when the phrase does not already contain it —
    // "sync lock acquired `lock`" is noise, but `cloned`, `collect`, `read`,
    // and `push` are exactly the distinctions D5 and §7.6 exist to keep.
    if !site.operation.is_empty() && !phrase.contains(site.operation.as_str()) {
        let _ = write!(text, " `{}`", cell(&site.operation));
    }
    if !site.operand.is_empty() {
        let _ = write!(
            text,
            " ({} `{}`)",
            operand_label(&site.kind),
            cell(&site.operand)
        );
    }
    let _ = write!(text, " at {}", location_text(site.location.as_ref()));
    text
}

fn evidence_text(evidence: &[EvidenceRef]) -> String {
    if evidence.is_empty() {
        return "unattributed (no evidence relation recorded)".to_string();
    }
    evidence
        .iter()
        .map(|e| format!("{} ({})", e.level, e.provider))
        .collect::<Vec<_>>()
        .join(", ")
}

fn capability_label(capability: &str) -> String {
    match capability {
        "ast_extraction" => "AST".to_string(),
        "type_inference" => "type inference".to_string(),
        "mir_lowering" => "MIR".to_string(),
        other => one_line(other),
    }
}

/// Render the §10 grouped explanation.
pub fn render_table(report: &OwnershipReport) -> String {
    let mut out = String::new();
    if report.state != OwnershipState::Matched {
        let _ = writeln!(out, "{}", report.message);
        return out;
    }

    for function in &report.functions {
        render_function(&mut out, function);
    }

    if report.returned_functions < report.matched_functions {
        let _ = writeln!(
            out,
            "{} of {} matching function(s); raise --limit for more.",
            report.returned_functions, report.matched_functions
        );
    } else {
        let _ = writeln!(out, "{} matching function(s).", report.matched_functions);
    }
    out
}

/// One function's full block: header, then each section separated by
/// a blank line, with a trailing blank line.
fn render_function(out: &mut String, function: &FunctionEvidence) {
    let _ = writeln!(out, "Function: {}", one_line(&function.function));
    out.push('\n');
    render_observed(out, function);
    out.push('\n');
    render_relationships(out, function);
    out.push('\n');
    render_evidence(out, function);
    out.push('\n');
    render_limits(out, function);
    out.push('\n');
}

/// The `Observed:` section — every indexed site with its lineage.
fn render_observed(out: &mut String, function: &FunctionEvidence) {
    let _ = writeln!(out, "Observed:");
    if function.sites.is_empty() {
        let _ = writeln!(out, "  (no indexed sites for this function)");
    }
    for site in &function.sites {
        let _ = writeln!(out, "  {}", describe_site(site));
        let _ = writeln!(out, "    evidence: {}", evidence_text(&site.evidence));
        if !site.resolved_types.is_empty() {
            let _ = writeln!(
                out,
                "    resolved type: {}",
                cell(&site.resolved_types.join(", "))
            );
        }
        for status in &site.analysis {
            let _ = writeln!(
                out,
                "    {}: {} ({})",
                capability_label(&status.capability),
                one_line(&status.status),
                one_line(&status.reason)
            );
        }
        for note in &site.notes {
            let _ = writeln!(out, "    note: {note}");
        }
    }
}

/// The `Relationships:` section — derived relations and their support.
fn render_relationships(out: &mut String, function: &FunctionEvidence) {
    let _ = writeln!(out, "Relationships:");
    if function.relationships.is_empty() {
        let _ = writeln!(out, "  (no derived relationships indexed)");
    }
    for relation in &function.relationships {
        let _ = writeln!(out, "  {}", one_line(&relation.relation));
        for support in &relation.supported_by {
            let _ = writeln!(out, "    {}: {}", support.role, support.summary);
        }
        let _ = writeln!(out, "    evidence: {}", evidence_text(&relation.evidence));
        let _ = writeln!(out, "    limit: {}", relation.limit);
    }
}

/// The `Evidence:` section — one line per known capability (reported or
/// not), then any capability the graph knows that this surface does not.
fn render_evidence(out: &mut String, function: &FunctionEvidence) {
    let _ = writeln!(out, "Evidence:");
    for capability in ANALYSIS_CAPABILITIES {
        let rows: Vec<&AnalysisStatus> = function
            .analysis
            .iter()
            .filter(|status| status.capability == *capability)
            .collect();
        if rows.is_empty() {
            let _ = writeln!(
                out,
                "  {}: not reported (no provider recorded a result)",
                capability_label(capability)
            );
            continue;
        }
        for status in rows {
            // Only name the subject when it is not the function itself —
            // a file-scoped status (a site cap, a stale compiler
            // generation) means something different from a function one.
            let scope = if status.subject == function.function {
                String::new()
            } else {
                format!(" for {}", cell(&status.subject))
            };
            let _ = writeln!(
                out,
                "  {}: {} ({}){scope}",
                capability_label(capability),
                one_line(&status.status),
                one_line(&status.reason),
            );
        }
    }
    for status in &function.analysis {
        if !ANALYSIS_CAPABILITIES.contains(&status.capability.as_str()) {
            let _ = writeln!(
                out,
                "  {}: {} ({})",
                capability_label(&status.capability),
                one_line(&status.status),
                one_line(&status.reason)
            );
        }
    }
}

/// The `Limit:` section.
fn render_limits(out: &mut String, function: &FunctionEvidence) {
    let _ = writeln!(out, "Limit:");
    for limit in &function.limits {
        let _ = writeln!(out, "  {limit}");
    }
}

/// Render the machine-readable form. Both surfaces serialize the same struct,
/// so there is no envelope for them to disagree about.
pub fn render_json(report: &OwnershipReport) -> String {
    serde_json::to_string_pretty(report).unwrap_or_else(|e| format!("{{\"error\":\"{e}\"}}"))
}
