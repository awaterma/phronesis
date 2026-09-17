//! Three-way starter rule sync. A separate baseline survives MCP rule saves.
use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;

use super::{InitError, InitOpts, InitReport, compose_packs, ensure_parent, with_extension};
use serde_json::{Value, json};

fn io_error(path: &Path, source: std::io::Error) -> InitError {
    InitError::Io {
        path: path.display().to_string(),
        source,
    }
}

fn read_rules(path: &Path) -> Result<Value, InitError> {
    let content = std::fs::read_to_string(path).map_err(|source| io_error(path, source))?;
    let value: Value = serde_json::from_str(&content)?;
    rule_map(&value)?;
    Ok(value)
}

fn rule_map(value: &Value) -> Result<BTreeMap<&str, &Value>, InitError> {
    let rules = value
        .get("rules")
        .and_then(Value::as_array)
        .ok_or_else(|| InitError::InvalidRules("expected an object with a rules array".into()))?;
    let mut result = BTreeMap::new();
    for rule in rules {
        serde_json::from_value::<crate::rules_file::SourceRule>(rule.clone())?;
        let id = rule
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .ok_or_else(|| InitError::InvalidRules("rule needs a nonempty id".into()))?;
        if result.insert(id, rule).is_some() {
            return Err(InitError::InvalidRules(format!("duplicate rule id: {id}")));
        }
    }
    Ok(result)
}

fn merge(
    existing: &mut Value,
    baseline: &Value,
    starter: &Value,
    report: &mut InitReport,
) -> Result<Value, InitError> {
    let previous = rule_map(baseline)?;
    let incoming = rule_map(starter)?;
    let current = rule_map(existing)?;
    let mut merged = Vec::new();
    let mut updated = 0;
    let mut added = 0;
    for rule in existing["rules"].as_array().into_iter().flatten() {
        let id = rule["id"].as_str().unwrap_or_default();
        match incoming.get(id) {
            Some(next) if previous.get(id) == Some(&rule) => {
                updated += usize::from(rule != *next);
                merged.push((*next).clone());
            }
            Some(next) if rule != *next => {
                report.warnings.push(format!(
                    "preserved local or untracked rule `{id}`; starter definition differs"
                ));
                merged.push(rule.clone());
            }
            _ => merged.push(rule.clone()),
        }
    }
    for rule in starter["rules"].as_array().into_iter().flatten() {
        let id = rule["id"].as_str().unwrap_or_default();
        // Absence of a previously installed rule is an intentional deletion.
        if !current.contains_key(id) && !previous.contains_key(id) {
            merged.push(rule.clone());
            added += 1;
        }
    }
    let mut recorded: Vec<_> = previous
        .iter()
        .filter(|(id, _)| !incoming.contains_key(**id))
        .map(|(_, rule)| (*rule).clone())
        .collect();
    recorded.extend(starter["rules"].as_array().into_iter().flatten().cloned());
    existing["rules"] = json!(merged);
    report.steps.push(format!(
        "~ rules sync: {updated} updated, {added} added; other rules preserved"
    ));
    Ok(json!({"rules": recorded}))
}

fn write_atomic(path: &Path, value: &Value) -> Result<(), InitError> {
    let parent = path
        .parent()
        .ok_or_else(|| InitError::InvalidRules("missing parent directory".into()))?;
    let mut temp =
        tempfile::NamedTempFile::new_in(parent).map_err(|source| io_error(path, source))?;
    if path.exists() {
        let metadata = std::fs::metadata(path).map_err(|source| io_error(path, source))?;
        temp.as_file()
            .set_permissions(metadata.permissions())
            .map_err(|source| io_error(path, source))?;
    }
    temp.write_all(serde_json::to_string_pretty(value)?.as_bytes())
        .map_err(|source| io_error(path, source))?;
    temp.persist(path)
        .map_err(|error| io_error(path, error.error))?;
    Ok(())
}

pub(super) fn write_rules_file(
    root: &Path,
    opts: &InitOpts,
    report: &mut InitReport,
) -> Result<(), InitError> {
    let path = root.join(".phronesis/rules.json");
    let baseline_path = root.join(".phronesis/starter-rules.json");
    if path.exists() && !opts.force && !opts.rules_only {
        report.steps.push("= .phronesis/rules.json already exists — leaving unchanged (use --rules-only to sync or --force to overwrite)".into());
        return Ok(());
    }
    let starter = compose_packs(&opts.packs);
    let mut rules = starter.clone();
    let mut baseline = starter;
    if path.exists() && !opts.force {
        rules = read_rules(&path)?;
        let previous = if baseline_path.exists() {
            read_rules(&baseline_path)?
        } else {
            json!({"rules": []})
        };
        baseline = merge(&mut rules, &previous, &baseline, report)?;
    }
    if opts.dry_run {
        report
            .steps
            .push("+ would write .phronesis/rules.json and starter-rules.json".into());
        return Ok(());
    }
    ensure_parent(&path)?;
    let unchanged = path.exists() && read_rules(&path).is_ok_and(|existing| existing == rules);
    if !unchanged {
        if path.exists() {
            let backup = with_extension(&path, "bak");
            std::fs::copy(&path, &backup).map_err(|source| io_error(&backup, source))?;
        }
        write_atomic(&path, &rules)?;
    }
    // Write rules first: a stale baseline makes retries preserve changed rules.
    write_atomic(&baseline_path, &baseline)?;
    let action = if unchanged { "= unchanged" } else { "+ wrote" };
    report.steps.push(format!(
        "{action} .phronesis/rules.json ({} rule(s)); recorded starter baseline",
        rules["rules"].as_array().map_or(0, Vec::len)
    ));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(id: &str, priority: u32) -> Value {
        json!({"id": id, "phase": "pre", "priority": priority, "when": [], "then": {"block": "test rule"}})
    }

    #[test]
    fn upgrades_only_unchanged_rules_and_preserves_deletions_and_metadata() {
        let baseline = json!({"rules": [rule("upgrade", 1), rule("edited", 1), rule("deleted", 1), rule("other-pack", 1)]});
        let mut current = json!({"metadata": {"owner": "example-app"}, "rules": [rule("upgrade", 1), rule("edited", 9), rule("custom", 7), rule("other-pack", 1)]});
        let starter = json!({"rules": [rule("upgrade", 2), rule("edited", 2), rule("deleted", 2), rule("new", 2)]});
        let mut report = InitReport::default();
        let next = merge(&mut current, &baseline, &starter, &mut report).unwrap();
        assert_eq!(
            current["rules"],
            json!([
                rule("upgrade", 2),
                rule("edited", 9),
                rule("custom", 7),
                rule("other-pack", 1),
                rule("new", 2)
            ])
        );
        assert_eq!(current["metadata"]["owner"], "example-app");
        assert_eq!(report.warnings.len(), 1);
        let once = current.clone();
        merge(&mut current, &next, &starter, &mut InitReport::default()).unwrap();
        assert_eq!(current, once);
    }

    #[test]
    fn untracked_collisions_are_not_overwritten() {
        let mut current = json!({"rules": [rule("same-id", 9)]});
        let starter = json!({"rules": [rule("same-id", 1)]});
        let baseline = json!({"rules": []});
        let mut report = InitReport::default();
        merge(&mut current, &baseline, &starter, &mut report).unwrap();
        assert_eq!(current["rules"][0]["priority"], 9);
        assert_eq!(report.warnings.len(), 1);
    }
}
