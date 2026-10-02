use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Arm {
    Control,
    Treatment,
}

impl Arm {
    pub fn as_str(self) -> &'static str {
        match self {
            Arm::Control => "control",
            Arm::Treatment => "treatment",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RunExit {
    Completed,
    CapTurns,
    CapTime,
    Error { reason: String },
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct AuditSummary {
    pub total_violations: u32,
    pub per_rule: BTreeMap<String, u32>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct GovernanceSummary {
    pub blocks: BTreeMap<String, u32>,
    pub warns: BTreeMap<String, u32>,
    pub fail_closed: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunRecord {
    pub instance_id: String,
    pub arm: Arm,
    pub exit: RunExit,
    pub resolved: Option<bool>,      // None until Task 9's verify fills it
    pub turns: u32,
    pub tokens_in: Option<u64>,      // None = router did not report usage
    pub tokens_out: Option<u64>,
    pub wall_clock_secs: u64,
    pub diff_bytes: u64,
    pub audit: Option<AuditSummary>, // None until Task 10's quality fills it
    pub governance: Option<GovernanceSummary>, // treatment arm only
}

/// The loud validator the aggregate step calls before trusting a record.
pub fn validate(r: &RunRecord) -> anyhow::Result<()> {
    if r.arm == Arm::Treatment && r.governance.is_none() {
        anyhow::bail!(
            "treatment record for {} is missing its governance field",
            r.instance_id
        );
    }
    if matches!(r.exit, RunExit::Error { .. }) && r.resolved == Some(true) {
        anyhow::bail!("record for {} errored yet claims resolved", r.instance_id);
    }
    Ok(())
}
