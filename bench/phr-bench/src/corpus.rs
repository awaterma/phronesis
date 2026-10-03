use crate::manifest::{packs_for, Caps, DatasetRef, Manifest, TaskSpec};
use anyhow::{bail, Context, Result};
use rand::seq::SliceRandom;
use rand::SeedableRng;
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slice {
    Pilot,
    Full,
}

impl Slice {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pilot => "pilot",
            Self::Full => "full",
        }
    }

    fn target(self) -> usize {
        match self {
            Self::Pilot => 30,
            Self::Full => 100,
        }
    }
}

pub fn build_manifest(
    dataset_jsonl: &str,
    slice: Slice,
    seed: u64,
    dataset: DatasetRef,
) -> Result<Manifest> {
    let mut groups = std::collections::BTreeMap::<String, Vec<TaskSpec>>::new();
    for (line_number, line) in dataset_jsonl.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let value: Value = serde_json::from_str(line)
            .with_context(|| format!("invalid dataset JSON on line {}", line_number + 1))?;
        let task = task_from_value(&value)
            .with_context(|| format!("invalid dataset row on line {}", line_number + 1))?;
        groups.entry(task.language.clone()).or_default().push(task);
    }
    if groups.is_empty() {
        bail!("dataset contains no instances");
    }

    let mut selected = groups.remove("rust").unwrap_or_default();
    let target = slice
        .target()
        .min(groups.values().map(Vec::len).sum::<usize>() + selected.len());
    if selected.len() < target {
        let capacity: usize = groups.values().map(Vec::len).sum();
        let slots = target - selected.len();
        let mut quotas = proportional_quotas(&groups, slots, capacity);
        let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
        for (language, tasks) in &mut groups {
            tasks.shuffle(&mut rng);
            selected.extend(
                tasks
                    .iter()
                    .take(quotas.remove(language).unwrap_or(0))
                    .cloned(),
            );
        }
    }
    selected.sort_by(|a, b| a.instance_id.cmp(&b.instance_id));
    let manifest = Manifest {
        dataset,
        seed,
        prompt_hash: crate::prompt::template_hash(),
        caps: Caps::default(),
        tasks: selected,
    };
    let encoded = serde_json::to_vec(&manifest).context("serialize manifest")?;
    let _: Manifest = serde_json::from_slice(&encoded).context("validate serialized manifest")?;
    Ok(manifest)
}

fn proportional_quotas(
    groups: &std::collections::BTreeMap<String, Vec<TaskSpec>>,
    slots: usize,
    capacity: usize,
) -> std::collections::BTreeMap<String, usize> {
    if capacity == 0 || slots == 0 {
        return groups.keys().map(|key| (key.clone(), 0)).collect();
    }
    let mut quotas = std::collections::BTreeMap::new();
    let mut remainders = Vec::new();
    let mut assigned = 0;
    for (language, tasks) in groups {
        let numerator = slots * tasks.len();
        let floor = (numerator / capacity).min(tasks.len());
        assigned += floor;
        quotas.insert(language.clone(), floor);
        remainders.push((language.clone(), numerator % capacity));
    }
    remainders.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    for (language, _) in remainders {
        if assigned == slots {
            break;
        }
        let quota = quotas.entry(language.clone()).or_default();
        if *quota < groups[&language].len() {
            *quota += 1;
            assigned += 1;
        }
    }
    quotas
}

fn task_from_value(value: &Value) -> Result<TaskSpec> {
    let string = |key: &str| -> Result<&str> {
        value
            .get(key)
            .and_then(Value::as_str)
            .with_context(|| format!("missing string field {key}"))
    };
    let repo_name = string("repo")?;
    let parser = string("log_parser")?;
    let language = if parser == "parse_log_cargo" {
        "rust"
    } else {
        language_from_id(string("instance_id")?)
            .or_else(|| language_from_repo(repo_name))
            .unwrap_or("unknown")
    };
    let list = |key: &str| -> Result<Vec<String>> {
        value
            .get(key)
            .and_then(Value::as_array)
            .with_context(|| format!("missing list field {key}"))?
            .iter()
            .map(|item| {
                item.as_str()
                    .map(str::to_owned)
                    .with_context(|| format!("{key} entries must be strings"))
            })
            .collect()
    };
    let repo = if repo_name.starts_with("https://") {
        repo_name.to_owned()
    } else {
        format!("https://github.com/{repo_name}.git")
    };
    Ok(TaskSpec {
        instance_id: string("instance_id")?.to_owned(),
        language: language.to_owned(),
        repo,
        base_commit: string("base_commit")?.to_owned(),
        issue_text: string("problem_statement")?.to_owned(),
        fail_to_pass: list("FAIL_TO_PASS")?,
        pass_to_pass: list("PASS_TO_PASS")?,
        packs: packs_for(language),
    })
}

fn language_from_id(instance_id: &str) -> Option<&'static str> {
    [
        "python",
        "typescript",
        "javascript",
        "rust",
        "java",
        "go",
        "ruby",
        "c",
        "cpp",
    ]
    .into_iter()
    .find(|language| instance_id.starts_with(&format!("{language}__")))
}

fn language_from_repo(repo: &str) -> Option<&'static str> {
    match repo.split('/').next()? {
        "psf" | "django" | "pallets" | "pytest-dev" | "scikit-learn" | "sympy" => Some("python"),
        "rust-lang" | "tokio-rs" | "BurntSushi" | "sharkdp" | "nushell" | "uutils" => Some("rust"),
        "microsoft" | "DefinitelyTyped" => Some("typescript"),
        "golang" => Some("go"),
        "ruby" => Some("ruby"),
        _ => None,
    }
}
