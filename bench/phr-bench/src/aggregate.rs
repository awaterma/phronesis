use crate::record::{Arm, RunRecord};
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskPair {
    pub instance_id: String,
    pub language: String,
    pub control: RunRecord,
    pub treatment: RunRecord,
    pub discordant: bool,
    pub treatment_won: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LangTotals {
    pub resolved_control: u32,
    pub resolved_treatment: u32,
    pub mean_turns_control: f64,
    pub mean_turns_treatment: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Headline {
    pub resolved_control: u32,
    pub resolved_treatment: u32,
    pub n: u32,
    pub discordant: u32,
    pub treatment_won: u32,
    pub control_won: u32,
    pub sign_p: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Aggregate {
    pub pairs: Vec<TaskPair>,
    pub by_language: BTreeMap<String, LangTotals>,
    pub headline: Headline,
}

/// Compute exact two-sided binomial p-value for sign test.
/// p = min(1.0, 2.0 * P(X ≤ min(wins, losses))) where X ~ Binomial(n=wins+losses, p=0.5)
pub fn sign_test_p(wins: u64, losses: u64) -> f64 {
    if wins + losses == 0 {
        return 1.0;
    }
    let n = wins + losses;
    let k = wins.min(losses);

    let mut pmf = 0.0;
    let mut binom_coeff = 1.0;

    for i in 0..=k {
        if i > 0 {
            binom_coeff *= (n - i + 1) as f64 / i as f64;
        }
        pmf += binom_coeff;
    }

    let p_one_sided = pmf / 2_f64.powi(n as i32);
    (2.0 * p_one_sided).min(1.0)
}

/// Aggregate pairs of control/treatment records, compute headline stats and sign test.
pub fn aggregate(records: &[RunRecord]) -> Result<Aggregate> {
    // Group by instance_id
    let mut by_id: BTreeMap<String, Vec<RunRecord>> = BTreeMap::new();
    for rec in records {
        by_id.entry(rec.instance_id.clone()).or_default().push(rec.clone());
    }

    // Validate: each instance must have exactly one control and one treatment
    let mut pairs = Vec::new();
    for (id, recs) in by_id {
        if recs.len() != 2 {
            bail!("instance {} has {} records, expected 2 (control + treatment)", id, recs.len());
        }

        let control = recs.iter().find(|r| r.arm == Arm::Control);
        let treatment = recs.iter().find(|r| r.arm == Arm::Treatment);

        if control.is_none() || treatment.is_none() {
            bail!("instance {} missing control or treatment arm", id);
        }

        let control = control.ok_or_else(|| anyhow::anyhow!("missing control"))?;
        let treatment = treatment.ok_or_else(|| anyhow::anyhow!("missing treatment"))?;

        // Validate both records
        crate::record::validate(control)?;
        crate::record::validate(treatment)?;

        let language = id.split('-').next().unwrap_or("unknown").to_string();

        let control_resolved = control.resolved.unwrap_or(false);
        let treatment_resolved = treatment.resolved.unwrap_or(false);
        let discordant = control_resolved != treatment_resolved;
        let treatment_won = treatment_resolved && !control_resolved;

        pairs.push(TaskPair {
            instance_id: id,
            language,
            control: control.clone(),
            treatment: treatment.clone(),
            discordant,
            treatment_won,
        });
    }

    // Compute headline stats
    let n = pairs.len() as u32;
    let mut resolved_control = 0u32;
    let mut resolved_treatment = 0u32;
    let mut discordant_count = 0u32;
    let mut treatment_won_count = 0u32;
    let mut control_won_count = 0u32;
    let mut by_language: BTreeMap<String, (u32, u32, u32, u32)> = BTreeMap::new();

    for pair in &pairs {
        if pair.control.resolved == Some(true) {
            resolved_control += 1;
        }
        if pair.treatment.resolved == Some(true) {
            resolved_treatment += 1;
        }
        if pair.discordant {
            discordant_count += 1;
            if pair.treatment_won {
                treatment_won_count += 1;
            } else {
                control_won_count += 1;
            }
        }

        let lang_entry = by_language
            .entry(pair.language.clone())
            .or_insert((0, 0, 0, 0));
        if pair.control.resolved == Some(true) {
            lang_entry.0 += 1;
        }
        if pair.treatment.resolved == Some(true) {
            lang_entry.1 += 1;
        }
        lang_entry.2 += pair.control.turns;
        lang_entry.3 += pair.treatment.turns;
    }

    let sign_p = sign_test_p(treatment_won_count as u64, control_won_count as u64);

    let headline = Headline {
        resolved_control,
        resolved_treatment,
        n,
        discordant: discordant_count,
        treatment_won: treatment_won_count,
        control_won: control_won_count,
        sign_p,
    };

    // Build by_language map
    let by_language = by_language
        .into_iter()
        .map(|(lang, (res_c, res_t, turns_c, turns_t))| {
            let count = pairs.iter().filter(|p| p.language == lang).count() as u32;
            (
                lang,
                LangTotals {
                    resolved_control: res_c,
                    resolved_treatment: res_t,
                    mean_turns_control: turns_c as f64 / count.max(1) as f64,
                    mean_turns_treatment: turns_t as f64 / count.max(1) as f64,
                },
            )
        })
        .collect();

    Ok(Aggregate {
        pairs,
        by_language,
        headline,
    })
}
