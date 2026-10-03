use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DatasetRef {
    pub id: String,
    pub revision: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Caps {
    pub max_turns: u32,
    pub max_wall_clock_secs: u64,
}

impl Default for Caps {
    fn default() -> Self {
        Self {
            max_turns: 100,
            max_wall_clock_secs: 45 * 60,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TaskSpec {
    pub instance_id: String,
    pub language: String, // lowercase dataset language, e.g. "rust"
    pub repo: String,     // GitHub URL
    pub base_commit: String,
    pub issue_text: String,
    pub fail_to_pass: Vec<String>,
    pub pass_to_pass: Vec<String>,
    pub packs: Vec<String>, // phr-mcp init packs, from the language map
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub dataset: DatasetRef,
    pub seed: u64,
    pub prompt_hash: String, // sha256 hex of the rendered template (Task 4)
    pub caps: Caps,
    pub tasks: Vec<TaskSpec>,
}

/// Language -> phr-mcp packs (spec: "Language -> pack map"). The map is total:
/// unknown languages fall back to ["llm"].
pub fn packs_for(language: &str) -> Vec<String> {
    match language {
        "rust" => vec!["llm", "rust"],
        "python" => vec!["llm", "python"],
        "typescript" | "javascript" => vec!["llm", "typescript"],
        _ => vec!["llm"],
    }
    .into_iter()
    .map(String::from)
    .collect()
}
