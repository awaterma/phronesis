//! Noncoding operator workflow over the governance core: the operator
//! authors two JSON files (a rules file and an events file) and runs one
//! command; no Rust code, no `.phronesis` directory, no project discovery,
//! explicit paths only. Run with:
//!   cargo run -p phronesis-governance --example operator_flow -- \
//!     <rules.json> <events.json> [--phase pre|post|audit]
//! Exit codes: 0 allow, 1 warn (proceed), 2 deny (block), 3 input error.

use std::collections::{BTreeSet, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use phr::Fact;
use phronesis_governance::decision::{Decision, Verdict};
use phronesis_governance::evaluation::{
    EvaluationError, FactSnapshot, builtin_evaluator, evaluate,
};
use phronesis_governance::{CompiledPolicy, Phase, PolicyError, Selection, parse};
use serde::Deserialize;

#[derive(Debug, thiserror::Error)]
enum WorkflowError {
    #[error("missing <rules.json> argument")]
    MissingRulesPath,
    #[error("missing <events.json> argument")]
    MissingEventsPath,
    #[error("unknown argument `{0}`")]
    UnknownArgument(String),
    #[error("unknown phase `{0}`; use pre, post, or audit")]
    BadPhase(String),
    #[error("--phase needs a value")]
    MissingPhaseValue,
    #[error("cannot read {what} {path}: {source}")]
    ReadFile {
        what: &'static str,
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("rules file rejected: {0}")]
    Rules(#[from] PolicyError),
    #[error("events file malformed: {0}")]
    Events(#[from] serde_json::Error),
    #[error("evaluation failed closed: {0}")]
    Evaluation(#[from] EvaluationError),
}

#[derive(Deserialize)]
struct OperatorFact {
    id: String,
    predicate: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    timestamp: u64,
    #[serde(default)]
    source: Option<String>,
}

impl OperatorFact {
    fn into_fact(self) -> Fact {
        Fact {
            id: self.id,
            predicate: self.predicate,
            args: self.args,
            timestamp: self.timestamp,
            source: self.source,
        }
    }
}

#[derive(Deserialize)]
struct OperatorEvents {
    #[serde(default)]
    host: Vec<OperatorFact>,
    #[serde(default)]
    extensions: Vec<OperatorFact>,
}

struct Arguments {
    rules_path: PathBuf,
    events_path: PathBuf,
    phase: Phase,
}

fn parse_arguments() -> Result<Arguments, WorkflowError> {
    let mut args = std::env::args().skip(1);
    let rules_path = args.next().ok_or(WorkflowError::MissingRulesPath)?;
    let events_path = args.next().ok_or(WorkflowError::MissingEventsPath)?;
    let mut phase = Phase::Pre;
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--phase" => {
                let value = args.next().ok_or(WorkflowError::MissingPhaseValue)?;
                phase = Phase::from_str_lossy(&value)
                    .filter(|p| !matches!(p, Phase::None))
                    .ok_or_else(|| WorkflowError::BadPhase(value.clone()))?;
            }
            other => return Err(WorkflowError::UnknownArgument(other.to_string())),
        }
    }
    Ok(Arguments {
        rules_path: PathBuf::from(rules_path),
        events_path: PathBuf::from(events_path),
        phase,
    })
}

fn read_file(path: &Path, what: &'static str) -> Result<String, WorkflowError> {
    fs::read_to_string(path).map_err(|source| WorkflowError::ReadFile {
        what,
        path: path.to_path_buf(),
        source,
    })
}

fn report(decision: &Decision) {
    println!("verdict: {:?}", decision.verdict);
    for message in decision.block_messages() {
        println!("block: {message}");
    }
    for message in decision.warning_messages() {
        println!("warning: {message}");
    }
    for diagnostic in &decision.diagnostics {
        println!("diagnostic: {diagnostic:?}");
    }
    for (reason, consequence) in decision.iter_reasons() {
        let rule = reason
            .rule_id
            .as_ref()
            .map(|id| id.to_string())
            .unwrap_or_else(|| "<no rule>".to_string());
        let bound_facts = match &consequence.provenance {
            phr::Provenance::RuleFiring { bound_facts, .. } => bound_facts.clone(),
            _ => vec![],
        };
        println!(
            "fired[{}] rule={} action={} severity={:?} bound={:?}",
            reason.index, rule, reason.action_type, reason.severity, bound_facts
        );
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let arguments = match parse_arguments() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("usage: operator_flow <rules.json> <events.json> [--phase pre|post|audit]");
            eprintln!("error: {e}");
            std::process::exit(3);
        }
    };

    let outcome = run(&arguments).await;
    match outcome {
        Ok(Verdict::Allow) => {
            println!("exit: 0 (allow)");
            std::process::exit(0);
        }
        Ok(Verdict::Warn) => {
            println!("exit: 1 (warn, proceed)");
            std::process::exit(1);
        }
        Ok(Verdict::Deny) => {
            println!("exit: 2 (deny, block)");
            std::process::exit(2);
        }
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(3);
        }
    }
}

async fn run(arguments: &Arguments) -> Result<Verdict, WorkflowError> {
    let rules_text = read_file(&arguments.rules_path, "rules file")?;
    let events_text = read_file(&arguments.events_path, "events file")?;

    let source_id = arguments
        .rules_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "operator-rules".to_string());

    let parsed = parse(&rules_text, &source_id)?;
    let policy = CompiledPolicy::compile(parsed.file.rules)?;
    for diagnostic in policy.diagnostics() {
        println!("policy diagnostic: {}", diagnostic.message());
    }
    println!(
        "loaded {} rule(s) from {} for the {} hook phase",
        policy.rules().len(),
        arguments.rules_path.display(),
        arguments.phase,
    );

    let events: OperatorEvents = serde_json::from_str(&events_text)?;
    let snapshot = FactSnapshot {
        host: events.host.into_iter().map(|f| f.into_fact()).collect(),
        extensions: events
            .extensions
            .into_iter()
            .map(|f| f.into_fact())
            .collect(),
    };

    let reserved: HashSet<String> = snapshot.host.iter().map(|f| f.predicate.clone()).collect();
    let demote = BTreeSet::new();

    let decision = evaluate(
        &policy,
        Selection::Hook(arguments.phase),
        &snapshot,
        &reserved,
        &demote,
        builtin_evaluator(),
    )
    .await?;

    report(&decision);
    Ok(decision.verdict)
}
