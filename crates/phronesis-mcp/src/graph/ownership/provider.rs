//! The compiler-aware evidence provider boundary (spec §8).
//!
//! Phase One ships **the interface plus availability reporting only**, which is
//! decision D10 and which spec §8.2 permits exactly when no stable structured
//! interface is used. Parsing human-formatted `analysis-stats` output is not a
//! production interface, so this provider makes no type, MIR, transfer, or
//! borrow claim at all: the only relation it can produce is
//! [`OWNERSHIP_ANALYSIS_STATUS`], and [`OwnershipEvidenceReport::to_edges`] is
//! the single place edges are built, so that limit is structural rather than a
//! matter of discipline.
//!
//! Three further properties are load-bearing:
//!
//! 1. **It cannot run implicitly.** [`RustAnalyzerProvider::for_rebuild`] is the
//!    only constructor and it yields `None` for every trigger except
//!    [`AnalysisTrigger::ExplicitRebuild`]. `pre-check`, `post-check`,
//!    hydration, and incremental single-file updates therefore have no value to
//!    call `analyze` on — §8.2 is enforced by the type system, not by a comment.
//! 2. **A missing binary is not an error.** §8.1 requires the AST extractor to
//!    stay fully usable with no provider installed, so an absent rust-analyzer
//!    is an ordinary `unavailable` / `tool_missing` observation and a rebuild
//!    continues normally. Nothing here ever executes the binary; detection is a
//!    `PATH` lookup.
//! 3. **Macro completeness is never claimed.** Build scripts and procedural
//!    macros are disabled by default (§8.2); when disabled the report carries an
//!    explicit [`ProviderLimitation`] for the rebuild diagnostics.

use super::config::{OwnershipConfig, OwnershipProvider};
use super::{OWNERSHIP_ANALYSIS_STATUS, PROVIDER_RUST_ANALYZER};
use crate::graph::model::Edge;
use std::path::{Path, PathBuf};

/// Optional override naming the rust-analyzer executable to detect.
///
/// Detection only ever *stats* this path; the binary is never executed.
pub const RUST_ANALYZER_PATH_ENV: &str = "PHRONESIS_RUST_ANALYZER";

/// One function the compiler-aware provider was asked about.
///
/// `file` is the repo-relative path the function was extracted from. It is not
/// decoration: it becomes the emitted edge's `src`, which is the only key
/// `store::compact` uses, so a status edge without it would be unreachable by
/// compaction and permanently stale (D12).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnershipFunction {
    /// Canonical graph function id, exactly as `defines_fn` spells it.
    pub id: String,
    /// Repo-relative source file containing the function.
    pub file: String,
}

impl OwnershipFunction {
    pub fn new(id: &str, file: &str) -> Self {
        Self {
            id: id.to_string(),
            file: file.to_string(),
        }
    }

    /// Whether this subject can produce a well-formed edge.
    ///
    /// An empty id or file has no resolvable subject or provenance, and
    /// `Edge::fact_id` joins arguments with U+001F, so an argument containing
    /// that byte would forge a different edge's identity. §7.11's rule applies
    /// to both: emit nothing.
    fn is_emittable(&self) -> bool {
        !self.id.is_empty()
            && !self.file.is_empty()
            && !self.id.contains('\u{1f}')
            && !self.file.contains('\u{1f}')
    }
}

/// Why the provider is being asked to run.
///
/// This exists so §8.2's "explicit graph rebuild only" is a value a caller must
/// supply rather than a rule a caller must remember. Every hook-time and
/// query-time path names its own trigger and is handed `None`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnalysisTrigger {
    /// `phr-mcp graph rebuild` (or the equivalent MCP tool) with compiler
    /// enrichment configured. The only trigger that may construct a provider.
    ExplicitRebuild,
    /// The `PreToolUse` hook.
    PreCheck,
    /// The `PostToolUse` hook.
    PostCheck,
    /// Turning persisted edges into RETE facts.
    Hydration,
    /// Re-extracting one edited file.
    IncrementalUpdate,
}

impl AnalysisTrigger {
    /// Whether compiler enrichment is permitted at all under this trigger.
    pub fn permits_compiler_enrichment(self) -> bool {
        matches!(self, Self::ExplicitRebuild)
    }
}

/// An analysis capability, from spec §6.1's closed set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnalysisCapability {
    AstExtraction,
    TypeInference,
    MirLowering,
}

impl AnalysisCapability {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AstExtraction => "ast_extraction",
            Self::TypeInference => "type_inference",
            Self::MirLowering => "mir_lowering",
        }
    }
}

/// An analysis status, from spec §6.1 as amended by D9 (`stale`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnalysisStatus {
    Available,
    Partial,
    Unavailable,
    Failed,
    Stale,
}

impl AnalysisStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Available => "available",
            Self::Partial => "partial",
            Self::Unavailable => "unavailable",
            Self::Failed => "failed",
            Self::Stale => "stale",
        }
    }
}

/// A stable machine reason (§6.1), never free-form stderr text.
///
/// Stderr is unstable across tool versions and can echo source text, so it is
/// unusable as a queryable argument. Anything the provider learns that does not
/// map to a value here is logged, not stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnalysisReason {
    /// MIR lowering failed because the body was async-lowered.
    AsyncLowering,
    /// No provider executable was found.
    ToolMissing,
    /// The provider could not load the project model.
    ProjectLoadFailed,
    /// The provider ran and failed for any other reason.
    ProviderError,
    /// Phase One exposes no stable structured interface for this capability,
    /// so the capability is unavailable even where the tool is installed. This
    /// is D10's scope limit made visible instead of silently absent.
    NoStructuredInterface,
    /// An incremental single-file edit invalidated compiler evidence (D9).
    IncrementalEdit,
}

impl AnalysisReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AsyncLowering => "async_lowering",
            Self::ToolMissing => "tool_missing",
            Self::ProjectLoadFailed => "project_load_failed",
            Self::ProviderError => "provider_error",
            Self::NoStructuredInterface => "no_structured_interface",
            Self::IncrementalEdit => "incremental_edit",
        }
    }
}

/// A bounded analysis the provider did **not** perform.
///
/// §8.2 requires that build scripts and procedural macros stay disabled by
/// default and that the limitation be recorded rather than papered over. These
/// are rebuild diagnostics, not graph edges: a limitation is a property of the
/// run, and encoding it as a per-function status would either contradict the
/// capability's real status or multiply edges by the function count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderLimitation {
    BuildScriptsDisabled,
    ProcMacrosDisabled,
}

impl ProviderLimitation {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::BuildScriptsDisabled => "build_scripts_disabled",
            Self::ProcMacrosDisabled => "proc_macros_disabled",
        }
    }
}

/// One `ownership_analysis_status` observation awaiting persistence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnalysisObservation {
    /// The function or file the status is about.
    pub subject: String,
    /// Repo-relative provenance file; becomes the edge's `src`.
    pub file: String,
    pub capability: AnalysisCapability,
    pub status: AnalysisStatus,
    pub reason: AnalysisReason,
}

impl AnalysisObservation {
    /// `ownership_analysis_status(subject, capability, status, reason)` as a
    /// **base** edge attributed to its source file (D12).
    pub fn to_edge(&self) -> Edge {
        Edge::base(
            OWNERSHIP_ANALYSIS_STATUS,
            &[
                &self.subject,
                self.capability.as_str(),
                self.status.as_str(),
                self.reason.as_str(),
            ],
            &self.file,
        )
    }
}

/// What a compiler-aware provider produced for one run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OwnershipEvidenceReport {
    observations: Vec<AnalysisObservation>,
    limitations: Vec<ProviderLimitation>,
}

impl OwnershipEvidenceReport {
    pub fn observations(&self) -> &[AnalysisObservation] {
        &self.observations
    }

    pub fn limitations(&self) -> &[ProviderLimitation] {
        &self.limitations
    }

    pub fn is_empty(&self) -> bool {
        self.observations.is_empty() && self.limitations.is_empty()
    }

    fn record(&mut self, observation: AnalysisObservation) {
        self.observations.push(observation);
    }

    fn limit(&mut self, limitation: ProviderLimitation) {
        if !self.limitations.contains(&limitation) {
            self.limitations.push(limitation);
        }
    }

    /// Stable machine strings for the rebuild diagnostics surface.
    pub fn diagnostics(&self) -> Vec<String> {
        self.limitations
            .iter()
            .map(|limitation| format!("{PROVIDER_RUST_ANALYZER}:{}", limitation.as_str()))
            .collect()
    }

    /// Every edge this report contributes to the graph.
    ///
    /// The only relation reachable from here is `ownership_analysis_status`.
    /// `resolved_type`, `ownership_transfer`, `borrow_live_across`, and every
    /// MIR relation are unreachable by construction in Phase One (D10).
    pub fn to_edges(&self) -> Vec<Edge> {
        self.observations
            .iter()
            .map(AnalysisObservation::to_edge)
            .collect()
    }
}

/// A provider failed in a way the caller may want to distinguish.
///
/// A *missing* provider is deliberately not in here: §8.1 requires the AST
/// extractor to work with no provider installed, so absence is reported as an
/// observation and never as an error that could fail a rebuild.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum OwnershipProviderError {
    #[error("ownership provider {provider} could not load the project at {root}")]
    ProjectLoadFailed { provider: String, root: String },
    #[error("ownership provider {provider} failed")]
    ProviderFailed { provider: String },
}

impl OwnershipProviderError {
    /// The stable machine reason this failure records.
    pub fn reason(&self) -> AnalysisReason {
        match self {
            Self::ProjectLoadFailed { .. } => AnalysisReason::ProjectLoadFailed,
            Self::ProviderFailed { .. } => AnalysisReason::ProviderError,
        }
    }
}

/// The compiler-aware evidence boundary (spec §8.1).
pub trait OwnershipEvidenceProvider {
    /// Provider name, as written into `ownership_evidence` and status edges.
    fn name(&self) -> &'static str;

    /// Analyze the given functions. Never invoked outside an explicit rebuild.
    fn analyze(
        &self,
        project_root: &Path,
        functions: &[OwnershipFunction],
    ) -> Result<OwnershipEvidenceReport, OwnershipProviderError>;
}

/// A provider failure turned into preserved evidence (§13.1).
///
/// AST observations already in the graph are untouched; the failure adds an
/// explicit `failed` status per capability so absence is never read as a clean
/// result (Goal 3).
pub fn failure_report(
    functions: &[OwnershipFunction],
    error: &OwnershipProviderError,
) -> OwnershipEvidenceReport {
    let mut report = OwnershipEvidenceReport::default();
    for function in functions.iter().filter(|f| f.is_emittable()) {
        for capability in [
            AnalysisCapability::TypeInference,
            AnalysisCapability::MirLowering,
        ] {
            report.record(AnalysisObservation {
                subject: function.id.clone(),
                file: function.file.clone(),
                capability,
                status: AnalysisStatus::Failed,
                reason: error.reason(),
            });
        }
    }
    report
}

/// D9: this file's compiler evidence, marked stale by an incremental edit.
///
/// Deliberately a free function rather than a provider method. No provider may
/// run on the incremental path (§8.2), so there is nothing to ask; the claim
/// being recorded is precisely that whatever a *previous* rebuild concluded no
/// longer describes these bytes. The subject is the file, not a function,
/// because the edit invalidates the whole file's generation and the function
/// set may itself have changed.
///
/// The observations carry `file` as their provenance, so `store::compact`
/// replaces any prior compiler status for this file with them (D12) — that is
/// what "replacing any prior compiler status" means mechanically, and it is
/// why nothing needs to search for the edges being superseded.
pub fn incremental_stale_report(file: &str) -> OwnershipEvidenceReport {
    let mut report = OwnershipEvidenceReport::default();
    let subject = OwnershipFunction::new(file, file);
    if !subject.is_emittable() {
        return report;
    }
    for capability in [
        AnalysisCapability::TypeInference,
        AnalysisCapability::MirLowering,
    ] {
        report.record(AnalysisObservation {
            subject: file.to_string(),
            file: file.to_string(),
            capability,
            status: AnalysisStatus::Stale,
            reason: AnalysisReason::IncrementalEdit,
        });
    }
    report
}

/// Whether the provider executable could be found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolAvailability {
    Present,
    Missing,
}

/// Whether macro-expanding analysis was enabled for a run.
///
/// Both default to off (§8.2). They are constructor arguments rather than
/// configuration keys because turning either on means running project code, and
/// that must be an explicit act at the call site.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MacroSupport {
    /// Off by default: enabling this runs the project's `build.rs`.
    pub build_scripts: bool,
    /// Off by default: enabling this runs procedural macro code.
    pub proc_macros: bool,
}

/// The rust-analyzer provider: availability reporting only (D10).
///
/// There is deliberately no public `new`. [`Self::for_rebuild`] is the only way
/// to obtain a value, so a hook or hydration path cannot call `analyze` even by
/// accident.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RustAnalyzerProvider {
    macros: MacroSupport,
}

impl RustAnalyzerProvider {
    /// A provider, if and only if this run may have one.
    ///
    /// `None` unless all three hold: the trigger is an explicit rebuild, the
    /// project enabled ownership enrichment, and it asked for the
    /// `rust-analyzer` provider. Every other combination — most importantly
    /// every hook-time trigger — gets nothing to call.
    pub fn for_rebuild(config: &OwnershipConfig, trigger: AnalysisTrigger) -> Option<Self> {
        if !trigger.permits_compiler_enrichment() {
            return None;
        }
        if !config.enabled || config.provider != OwnershipProvider::RustAnalyzer {
            return None;
        }
        Some(Self {
            macros: MacroSupport::default(),
        })
    }

    /// Explicitly opt this run into build scripts and/or procedural macros.
    ///
    /// Off by default; enabling either means running project code, which §3
    /// forbids doing implicitly.
    pub fn with_macro_support(mut self, macros: MacroSupport) -> Self {
        self.macros = macros;
        self
    }

    /// Locate the executable without running it.
    ///
    /// Detection is a `PATH` (or [`RUST_ANALYZER_PATH_ENV`]) lookup. Executing
    /// the binary to ask whether it exists would violate §3's "no Cargo, build
    /// scripts, or procedural macros implicitly" and would make detection cost
    /// a process spawn.
    pub fn locate() -> Option<PathBuf> {
        if let Some(explicit) = std::env::var_os(RUST_ANALYZER_PATH_ENV) {
            let path = PathBuf::from(explicit);
            return path.is_file().then_some(path);
        }
        let path_var = std::env::var_os("PATH")?;
        std::env::split_paths(&path_var)
            .flat_map(|dir| {
                ["rust-analyzer", "rust-analyzer.exe"]
                    .into_iter()
                    .map(move |name| dir.join(name))
            })
            .find(|candidate| candidate.is_file())
    }

    /// Detected availability for this machine.
    pub fn availability() -> ToolAvailability {
        match Self::locate() {
            Some(_) => ToolAvailability::Present,
            None => ToolAvailability::Missing,
        }
    }

    /// The report for a given availability, with no detection and no I/O.
    ///
    /// Split out from [`Self::analyze`] so both branches are testable on any
    /// machine — a test that only exercised whichever branch the developer's
    /// `PATH` happens to produce would pin nothing.
    ///
    /// Both compiler capabilities are `unavailable` in Phase One. Where the
    /// tool is missing the reason is `tool_missing`; where it is installed the
    /// reason is `no_structured_interface`, because this round exposes no
    /// stable structured interface to ask it through (§8.2, D10). Reporting
    /// `available` for an installed tool whose results are never consumed would
    /// be the exact "absence read as a clean result" failure Goal 3 forbids.
    pub fn report_for(
        &self,
        functions: &[OwnershipFunction],
        availability: ToolAvailability,
    ) -> OwnershipEvidenceReport {
        let reason = match availability {
            ToolAvailability::Missing => AnalysisReason::ToolMissing,
            ToolAvailability::Present => AnalysisReason::NoStructuredInterface,
        };
        let mut report = OwnershipEvidenceReport::default();
        for function in functions.iter().filter(|f| f.is_emittable()) {
            for capability in [
                AnalysisCapability::TypeInference,
                AnalysisCapability::MirLowering,
            ] {
                report.record(AnalysisObservation {
                    subject: function.id.clone(),
                    file: function.file.clone(),
                    capability,
                    status: AnalysisStatus::Unavailable,
                    reason,
                });
            }
        }
        if !self.macros.build_scripts {
            report.limit(ProviderLimitation::BuildScriptsDisabled);
        }
        if !self.macros.proc_macros {
            report.limit(ProviderLimitation::ProcMacrosDisabled);
        }
        report
    }
}

impl OwnershipEvidenceProvider for RustAnalyzerProvider {
    fn name(&self) -> &'static str {
        PROVIDER_RUST_ANALYZER
    }

    fn analyze(
        &self,
        _project_root: &Path,
        functions: &[OwnershipFunction],
    ) -> Result<OwnershipEvidenceReport, OwnershipProviderError> {
        Ok(self.report_for(functions, Self::availability()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::ownership::{ANALYSIS_CAPABILITIES, ANALYSIS_STATUSES, OWNERSHIP_RELATIONS};

    fn enabled(provider: OwnershipProvider) -> OwnershipConfig {
        OwnershipConfig {
            enabled: true,
            provider,
            ..OwnershipConfig::disabled()
        }
    }

    fn functions() -> Vec<OwnershipFunction> {
        vec![OwnershipFunction::new(
            "rust:demo::llm::scheduler::Scheduler::acquire",
            "src/llm/scheduler.rs",
        )]
    }

    // Pins §8.2 structurally: the provider is unconstructible on every
    // implicit path, so a hook, hydration, or incremental update physically
    // has no value to invoke. A comment saying "don't call this here" would
    // survive exactly until the first careless call site.
    #[test]
    fn no_trigger_except_an_explicit_rebuild_can_construct_the_provider() {
        let config = enabled(OwnershipProvider::RustAnalyzer);
        for trigger in [
            AnalysisTrigger::PreCheck,
            AnalysisTrigger::PostCheck,
            AnalysisTrigger::Hydration,
            AnalysisTrigger::IncrementalUpdate,
        ] {
            assert!(
                RustAnalyzerProvider::for_rebuild(&config, trigger).is_none(),
                "{trigger:?} must not be able to construct a compiler provider"
            );
        }
        assert!(
            RustAnalyzerProvider::for_rebuild(&config, AnalysisTrigger::ExplicitRebuild).is_some(),
            "an explicit rebuild with provider = rust-analyzer must get a provider"
        );
    }

    // Pins that opting in is required twice over: the feature must be enabled
    // and must name this provider. `provider = "ast"` requests AST extraction
    // only, and silently running rust-analyzer for it would break §9.
    #[test]
    fn configuration_must_both_enable_ownership_and_name_this_provider() {
        assert!(
            RustAnalyzerProvider::for_rebuild(
                &OwnershipConfig::disabled(),
                AnalysisTrigger::ExplicitRebuild
            )
            .is_none(),
            "disabled ownership must not construct a compiler provider"
        );
        assert!(
            RustAnalyzerProvider::for_rebuild(
                &enabled(OwnershipProvider::Ast),
                AnalysisTrigger::ExplicitRebuild
            )
            .is_none(),
            "provider = ast must not construct a compiler provider"
        );
    }

    // Pins D9. Nothing in the codebase constructed this edge before, so §9's
    // "mark compiler evidence stale rather than mixing generations" had no
    // representation at all: an edited file kept the previous rebuild's
    // compiler statuses verbatim and they read as current.
    #[test]
    fn an_incremental_edit_marks_both_compiler_capabilities_stale_against_the_file() {
        let report = incremental_stale_report("src/scheduler.rs");
        let rows: Vec<(&str, &str, &str, &str)> = report
            .observations()
            .iter()
            .map(|o| {
                (
                    o.subject.as_str(),
                    o.capability.as_str(),
                    o.status.as_str(),
                    o.reason.as_str(),
                )
            })
            .collect();
        assert_eq!(
            rows,
            vec![
                (
                    "src/scheduler.rs",
                    "type_inference",
                    "stale",
                    "incremental_edit"
                ),
                (
                    "src/scheduler.rs",
                    "mir_lowering",
                    "stale",
                    "incremental_edit"
                ),
            ],
            "D9 names the file as subject for both compiler capabilities"
        );
        assert!(
            report
                .to_edges()
                .iter()
                .all(|edge| edge.src == "src/scheduler.rs" && !edge.d),
            "the staleness edges must be base edges keyed on the edited file, or compaction can never replace them (D12)"
        );
        assert!(
            report.limitations().is_empty(),
            "a staleness marker is not a provider run and reports no provider limitation"
        );
    }

    // Pins §8.1: a machine with no rust-analyzer gets an ordinary observation,
    // never an error. If absence could fail a rebuild, the AST extractor would
    // stop being usable without the optional tool.
    #[test]
    fn a_missing_binary_is_a_status_observation_and_never_an_error() {
        let provider = RustAnalyzerProvider::for_rebuild(
            &enabled(OwnershipProvider::RustAnalyzer),
            AnalysisTrigger::ExplicitRebuild,
        )
        .expect("explicit rebuild constructs a provider");
        let report = provider.report_for(&functions(), ToolAvailability::Missing);
        let reasons: Vec<&str> = report
            .observations()
            .iter()
            .map(|observation| observation.reason.as_str())
            .collect();
        assert_eq!(
            reasons,
            vec!["tool_missing", "tool_missing"],
            "both compiler capabilities report the missing tool"
        );
        assert!(
            report
                .observations()
                .iter()
                .all(|observation| observation.status == AnalysisStatus::Unavailable),
            "a missing tool is unavailable, not failed"
        );
        // The same call through the trait must also succeed on any machine,
        // whether or not rust-analyzer happens to be installed here.
        let live = provider
            .analyze(Path::new("."), &functions())
            .expect("detection must never fail a rebuild");
        assert!(
            live.observations().iter().all(|observation| matches!(
                observation.reason,
                AnalysisReason::ToolMissing | AnalysisReason::NoStructuredInterface
            )),
            "availability-only analysis reports one of the two detection reasons"
        );
    }

    // Pins D10's whole scope: an installed rust-analyzer changes the *reason*
    // and nothing else. Reporting `available` for a tool this round never asks
    // anything of would make weak evidence look strong (Goal 3, Addendum A.4).
    #[test]
    fn an_installed_binary_still_reports_unavailable_with_the_phase_limit_reason() {
        let provider = RustAnalyzerProvider::for_rebuild(
            &enabled(OwnershipProvider::RustAnalyzer),
            AnalysisTrigger::ExplicitRebuild,
        )
        .expect("provider");
        let report = provider.report_for(&functions(), ToolAvailability::Present);
        assert!(
            report.observations().iter().all(|observation| {
                observation.status == AnalysisStatus::Unavailable
                    && observation.reason == AnalysisReason::NoStructuredInterface
            }),
            "an installed tool without a structured interface is still unavailable"
        );
        let capabilities: Vec<&str> = report
            .observations()
            .iter()
            .map(|observation| observation.capability.as_str())
            .collect();
        assert_eq!(
            capabilities,
            vec!["type_inference", "mir_lowering"],
            "both compiler capabilities are reported explicitly"
        );
    }

    // Pins D10's emission limit. This is the single worst failure the feature
    // can have — a compiler-strength claim produced by code that consulted no
    // compiler — so it is asserted over the edges themselves, not the intent.
    #[test]
    fn the_provider_can_emit_no_relation_but_ownership_analysis_status() {
        let provider = RustAnalyzerProvider::for_rebuild(
            &enabled(OwnershipProvider::RustAnalyzer),
            AnalysisTrigger::ExplicitRebuild,
        )
        .expect("provider");
        let mut edges = provider
            .report_for(&functions(), ToolAvailability::Present)
            .to_edges();
        edges.extend(
            provider
                .report_for(&functions(), ToolAvailability::Missing)
                .to_edges(),
        );
        edges.extend(
            failure_report(
                &functions(),
                &OwnershipProviderError::ProviderFailed {
                    provider: PROVIDER_RUST_ANALYZER.to_string(),
                },
            )
            .to_edges(),
        );
        assert!(!edges.is_empty(), "the fixture must produce edges to check");
        for edge in &edges {
            assert_eq!(
                edge.p, OWNERSHIP_ANALYSIS_STATUS,
                "the compiler provider may emit no other relation in Phase One"
            );
        }
        for forbidden in [
            "resolved_type",
            "ownership_transfer",
            "borrow_live_across",
            "lock_scope_may_cross_await",
            "ownership_conflict_diagnostic",
        ] {
            assert!(
                !edges.iter().any(|edge| edge.p == forbidden),
                "{forbidden} must be unreachable from the availability-only provider"
            );
        }
    }

    // Pins D12: an ownership edge with an empty `src` is invisible to
    // provenance-keyed compaction and becomes permanently stale, and a derived
    // edge passed in as fresh is silently discarded by `store::compact`.
    #[test]
    fn every_emitted_edge_is_a_base_edge_carrying_its_source_file() {
        let provider = RustAnalyzerProvider::for_rebuild(
            &enabled(OwnershipProvider::RustAnalyzer),
            AnalysisTrigger::ExplicitRebuild,
        )
        .expect("provider");
        let edges = provider
            .report_for(&functions(), ToolAvailability::Missing)
            .to_edges();
        for edge in &edges {
            assert!(!edge.d, "ownership edges are base edges, never derived");
            assert_eq!(
                edge.src, "src/llm/scheduler.rs",
                "src must be the repo-relative file so compaction can reach it"
            );
            assert_eq!(
                edge.a.len(),
                4,
                "status arity is [subject, cap, status, reason]"
            );
            assert_eq!(
                edge.a[0], "rust:demo::llm::scheduler::Scheduler::acquire",
                "subject is the canonical function id"
            );
            assert!(
                edge.a.iter().all(|arg| !arg.contains('\u{1f}')),
                "no argument may contain the fact-id separator"
            );
        }
    }

    // Pins §7.11 for this provider: a subject with no id, no provenance file,
    // or a separator byte in either cannot produce a well-formed edge, so it
    // produces none rather than a malformed one nothing will ever match.
    #[test]
    fn a_subject_without_a_usable_id_or_file_emits_nothing() {
        let provider = RustAnalyzerProvider::for_rebuild(
            &enabled(OwnershipProvider::RustAnalyzer),
            AnalysisTrigger::ExplicitRebuild,
        )
        .expect("provider");
        let unusable = vec![
            OwnershipFunction::new("", "src/lib.rs"),
            OwnershipFunction::new("rust:demo::f", ""),
            OwnershipFunction::new("rust:demo\u{1f}::f", "src/lib.rs"),
            OwnershipFunction::new("rust:demo::f", "src/li\u{1f}b.rs"),
        ];
        assert!(
            provider
                .report_for(&unusable, ToolAvailability::Missing)
                .to_edges()
                .is_empty(),
            "no unusable subject may reach the graph"
        );
    }

    // Pins §8.2's macro clause: with build scripts and proc macros off — the
    // default — the run records the limitation, so nothing downstream can read
    // its output as macro-complete analysis.
    #[test]
    fn disabled_build_scripts_and_proc_macros_are_recorded_as_limitations() {
        let provider = RustAnalyzerProvider::for_rebuild(
            &enabled(OwnershipProvider::RustAnalyzer),
            AnalysisTrigger::ExplicitRebuild,
        )
        .expect("provider");
        let report = provider.report_for(&functions(), ToolAvailability::Present);
        assert_eq!(
            report.limitations(),
            [
                ProviderLimitation::BuildScriptsDisabled,
                ProviderLimitation::ProcMacrosDisabled
            ],
            "both are disabled by default and both are recorded"
        );
        assert_eq!(
            report.diagnostics(),
            vec![
                "rust_analyzer:build_scripts_disabled".to_string(),
                "rust_analyzer:proc_macros_disabled".to_string()
            ],
            "diagnostics are stable machine strings"
        );
        let opted_in = provider.with_macro_support(MacroSupport {
            build_scripts: true,
            proc_macros: true,
        });
        assert!(
            opted_in
                .report_for(&functions(), ToolAvailability::Present)
                .limitations()
                .is_empty(),
            "an explicit opt-in clears the limitation it authorises"
        );
    }

    // Pins §13.1: a provider failure is recorded as evidence rather than
    // discarded, and it never borrows a reason from stderr.
    #[test]
    fn a_provider_failure_records_failed_status_with_a_stable_reason() {
        let error = OwnershipProviderError::ProjectLoadFailed {
            provider: PROVIDER_RUST_ANALYZER.to_string(),
            root: "/repo".to_string(),
        };
        let report = failure_report(&functions(), &error);
        assert!(
            report.observations().iter().all(|observation| {
                observation.status == AnalysisStatus::Failed
                    && observation.reason == AnalysisReason::ProjectLoadFailed
            }),
            "a load failure is failed/project_load_failed for both capabilities"
        );
        assert_eq!(
            OwnershipProviderError::ProviderFailed {
                provider: PROVIDER_RUST_ANALYZER.to_string()
            }
            .reason(),
            AnalysisReason::ProviderError,
            "a generic failure maps to provider_error"
        );
    }

    // Pins that every value this module can write into an edge is inside the
    // closed sets in `ownership::mod`. A value outside them is not a new
    // feature; it is a fact nothing queries.
    #[test]
    fn every_emitted_value_belongs_to_the_closed_sets() {
        for capability in [
            AnalysisCapability::AstExtraction,
            AnalysisCapability::TypeInference,
            AnalysisCapability::MirLowering,
        ] {
            assert!(
                ANALYSIS_CAPABILITIES.contains(&capability.as_str()),
                "{} must be a declared capability",
                capability.as_str()
            );
        }
        for status in [
            AnalysisStatus::Available,
            AnalysisStatus::Partial,
            AnalysisStatus::Unavailable,
            AnalysisStatus::Failed,
            AnalysisStatus::Stale,
        ] {
            assert!(
                ANALYSIS_STATUSES.contains(&status.as_str()),
                "{} must be a declared status",
                status.as_str()
            );
        }
        assert!(
            OWNERSHIP_RELATIONS.contains(&OWNERSHIP_ANALYSIS_STATUS),
            "the only relation this provider emits must be registered"
        );
        // §6.1 names four reasons; D9 and D10 add the two this round needs.
        // The set is closed either way — free-form text is what is banned.
        for reason in [
            AnalysisReason::AsyncLowering,
            AnalysisReason::ToolMissing,
            AnalysisReason::ProjectLoadFailed,
            AnalysisReason::ProviderError,
            AnalysisReason::NoStructuredInterface,
            AnalysisReason::IncrementalEdit,
        ] {
            let text = reason.as_str();
            assert!(
                !text.is_empty()
                    && text
                        .bytes()
                        .all(|byte| byte.is_ascii_lowercase() || byte == b'_'),
                "{text} must be a stable snake_case machine value"
            );
        }
    }
}
