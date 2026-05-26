use std::io::Write as IoWrite;
use std::process::{Command, Stdio};

use cucumber::{World as _, given, then, when};
use phr::{Action, Condition, Fact, ReteNetwork, Rule};
use phronesis_mcp::server::extract_rules_from_markdown;

// ---------------------------------------------------------------------------
// World — shared test state across Given/When/Then steps
// ---------------------------------------------------------------------------

#[derive(Debug, cucumber::World)]
struct World {
    network: ReteNetwork,
    last_json: String,
    last_actions: Vec<Action>,
    extracted_rules: Vec<Rule>,
    markdown_content: String,
    temp_dir: Option<tempfile::TempDir>,
    temp_file_path: Option<String>,
    rules_dir: Option<tempfile::TempDir>,
    checked_file_path: Option<String>,
    last_exit_code: Option<i32>,
    last_stderr: String,
}

impl Default for World {
    fn default() -> Self {
        Self {
            network: ReteNetwork::new(),
            last_json: String::new(),
            last_actions: Vec::new(),
            extracted_rules: Vec::new(),
            markdown_content: String::new(),
            temp_dir: None,
            temp_file_path: None,
            rules_dir: None,
            checked_file_path: None,
            last_exit_code: None,
            last_stderr: String::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn run_hook(subcommand: &str, payload: &str, cwd: Option<&str>) -> (i32, String) {
    let bin = env!("CARGO_BIN_EXE_phr-mcp");
    let mut cmd = Command::new(bin);
    cmd.arg(subcommand)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    let mut child = cmd.spawn().expect("failed to spawn hook");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(payload.as_bytes())
        .unwrap();
    let output = child.wait_with_output().expect("failed to wait");
    let code = output.status.code().unwrap_or(-1);
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    (code, stderr)
}

fn simple_rule(
    id: &str,
    priority: i32,
    cond_pred: &str,
    cond_arg: &str,
    action_type: &str,
    action_param: &str,
) -> Rule {
    Rule {
        id: id.to_string(),
        priority,
        conditions: vec![Condition {
            predicate: cond_pred.to_string(),
            args: vec![cond_arg.to_string()],
            script: None,
        }],
        actions: vec![Action {
            action_type: action_type.to_string(),
            params: vec![action_param.to_string()],
        }],
    }
}

// ---------------------------------------------------------------------------
// Given steps
// ---------------------------------------------------------------------------

#[given("a fresh RETE network")]
async fn fresh_network(world: &mut World) {
    *world = World::default();
}

#[given(expr = "the following rules are loaded")]
async fn load_rules_table(world: &mut World, step: &cucumber::gherkin::Step) {
    if let Some(table) = step.table.as_ref() {
        for row in table.rows.iter().skip(1) {
            let id = &row[0];
            let priority: i32 = row[1].parse().unwrap();
            let rule = simple_rule(id, priority, "placeholder", "value", "log", "matched");
            world.network.add_rule(rule).await.unwrap();
        }
    }
}

#[given(expr = "the following facts are asserted")]
async fn assert_facts_table(world: &mut World, step: &cucumber::gherkin::Step) {
    if let Some(table) = step.table.as_ref() {
        for row in table.rows.iter().skip(1) {
            let fact = Fact {
                id: row[0].clone(),
                predicate: row[1].clone(),
                args: vec![row[2].clone()],
                timestamp: 0,
            };
            world.network.assert_fact(fact).await.unwrap();
        }
    }
}

#[given(expr = "a rule {string} that checks for {string} in content")]
async fn given_content_check_rule(world: &mut World, rule_id: String, pattern: String) {
    let rule = simple_rule(
        &rule_id,
        10,
        "new_content_contains",
        &pattern,
        "constraint_violation",
        "Content contains forbidden pattern",
    );
    world.network.add_rule(rule).await.unwrap();
}

#[given(expr = "a fact asserting new_content_contains {string}")]
async fn given_content_fact(world: &mut World, content: String) {
    let fact = Fact {
        id: "content-fact".to_string(),
        predicate: "new_content_contains".to_string(),
        args: vec![content],
        timestamp: 0,
    };
    world.network.assert_fact(fact).await.unwrap();
}

#[given(expr = "a markdown document containing")]
async fn given_markdown_content(world: &mut World, step: &cucumber::gherkin::Step) {
    world.markdown_content = step.docstring.clone().unwrap_or_default();
}

#[given("a markdown file on disk at a temp path with content")]
async fn given_markdown_file_on_disk(world: &mut World, step: &cucumber::gherkin::Step) {
    let content = step.docstring.clone().unwrap_or_default();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test-rules.md");
    std::fs::write(&path, &content).unwrap();
    world.temp_file_path = Some(path.to_string_lossy().to_string());
    world.temp_dir = Some(dir);
}

#[given(expr = "a rules file with a pre-check rule blocking {string}")]
async fn given_pre_check_rules_file(world: &mut World, pattern: String) {
    let dir = tempfile::tempdir().unwrap();
    let phronesis_dir = dir.path().join(".phronesis");
    std::fs::create_dir_all(&phronesis_dir).unwrap();
    let rules = serde_json::json!({
        "rules": [{
            "id": "block-pattern",
            "phase": "pre",
            "priority": 10,
            "conditions": [
                { "predicate": "new_content_contains", "args": [pattern] }
            ],
            "actions": [
                { "action_type": "constraint_violation", "params": ["Blocked: forbidden pattern detected"] }
            ]
        }]
    });
    std::fs::write(
        phronesis_dir.join("rules.json"),
        serde_json::to_string_pretty(&rules).unwrap(),
    )
    .unwrap();
    world.rules_dir = Some(dir);
}

#[given(expr = "a rules file with a post-check rule requiring {string}")]
async fn given_post_check_rules_file(world: &mut World, required_pattern: String) {
    let dir = if let Some(dir) = world.rules_dir.take() {
        dir
    } else {
        tempfile::tempdir().unwrap()
    };
    let phronesis_dir = dir.path().join(".phronesis");
    std::fs::create_dir_all(&phronesis_dir).unwrap();
    let rules = serde_json::json!({
        "rules": [{
            "id": "require-pattern",
            "phase": "post",
            "priority": 10,
            "conditions": [
                { "predicate": "file_missing_pattern", "args": [required_pattern] }
            ],
            "actions": [
                { "action_type": "constraint_violation", "params": ["Required pattern missing from file"] }
            ]
        }]
    });
    std::fs::write(
        phronesis_dir.join("rules.json"),
        serde_json::to_string_pretty(&rules).unwrap(),
    )
    .unwrap();
    world.rules_dir = Some(dir);
}

#[given(expr = "a file on disk at the checked path without {string}")]
async fn given_file_without_pattern(world: &mut World, _pattern: String) {
    let dir = world.rules_dir.as_ref().expect("rules_dir must be set");
    let file_path = dir.path().join("schema.rs");
    std::fs::write(&file_path, "fn main() {}\n").unwrap();
    world.checked_file_path = Some(file_path.to_string_lossy().to_string());
}

#[given("a TDD-style rule binding ?file and ?fn")]
async fn given_tdd_rule(world: &mut World) {
    let rule = Rule {
        id: "tdd-style".to_string(),
        priority: 10,
        conditions: vec![Condition {
            predicate: "function_added".to_string(),
            args: vec!["?file".to_string(), "?fn".to_string()],
            script: None,
        }],
        actions: vec![Action {
            action_type: "constraint_violation".to_string(),
            params: vec![
                "Write a failing test for `?fn` before implementing it in ?file".to_string(),
            ],
        }],
    };
    world.network.add_rule(rule).await.unwrap();
}

#[given(expr = "a fact {string} with args {string} and {string}")]
async fn given_fact_with_two_args(world: &mut World, predicate: String, a1: String, a2: String) {
    let fact = Fact {
        id: format!("{}-fact", predicate),
        predicate,
        args: vec![a1, a2],
        timestamp: 0,
    };
    world.network.assert_fact(fact).await.unwrap();
}

#[then(expr = "the fired action message should equal {string}")]
async fn then_action_message_equals(world: &mut World, expected: String) {
    let result: serde_json::Value = serde_json::from_str(&world.last_json).unwrap();
    let actual = result["actions"][0]["params"][0]
        .as_str()
        .unwrap_or_default();
    assert_eq!(actual, expected);
}

#[given(expr = "a rule {string} with markdown_rule condition for {string} \\/ {string}")]
async fn given_markdown_rule(world: &mut World, id: String, file: String, section: String) {
    let rule = Rule {
        id,
        priority: 5,
        conditions: vec![Condition {
            predicate: "markdown_rule".to_string(),
            args: vec![file, section],
            script: None,
        }],
        actions: vec![Action {
            action_type: "constraint_violation".to_string(),
            params: vec!["section reminder".to_string()],
        }],
    };
    world.network.add_rule(rule).await.unwrap();
}

#[given("a malformed rules file")]
async fn given_malformed_rules_file(world: &mut World) {
    let dir = tempfile::tempdir().unwrap();
    let phronesis = dir.path().join(".phronesis");
    std::fs::create_dir_all(&phronesis).unwrap();
    std::fs::write(phronesis.join("rules.json"), "{not valid json").unwrap();
    world.rules_dir = Some(dir);
}

// ---------------------------------------------------------------------------
// When steps
// ---------------------------------------------------------------------------

#[when(expr = "I add a rule {string} with priority {int}")]
async fn when_add_rule(
    world: &mut World,
    id: String,
    priority: i32,
    step: &cucumber::gherkin::Step,
) {
    let mut conditions = Vec::new();
    if let Some(table) = step.table.as_ref() {
        for row in table.rows.iter().skip(1) {
            conditions.push(Condition {
                predicate: row[0].clone(),
                args: vec![row[1].clone()],
                script: None,
            });
        }
    }
    // Store pending state for the follow-up "has actions" step
    world.last_json = serde_json::to_string(&serde_json::json!({
        "id": id,
        "priority": priority,
        "conditions": conditions.iter().map(|c| {
            serde_json::json!({"predicate": c.predicate, "args": c.args})
        }).collect::<Vec<_>>(),
    }))
    .unwrap();
}

#[when("the rule has actions")]
async fn when_rule_has_actions(world: &mut World, step: &cucumber::gherkin::Step) {
    let pending: serde_json::Value = serde_json::from_str(&world.last_json).unwrap();
    let id = pending["id"].as_str().unwrap().to_string();
    let priority = pending["priority"].as_i64().unwrap() as i32;

    let conditions: Vec<Condition> = pending["conditions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| Condition {
            predicate: c["predicate"].as_str().unwrap().to_string(),
            args: c["args"]
                .as_array()
                .unwrap()
                .iter()
                .map(|a| a.as_str().unwrap().to_string())
                .collect(),
            script: None,
        })
        .collect();

    let mut actions = Vec::new();
    if let Some(table) = step.table.as_ref() {
        for row in table.rows.iter().skip(1) {
            actions.push(Action {
                action_type: row[0].clone(),
                params: vec![row[1].clone()],
            });
        }
    }

    let rule = Rule {
        id,
        priority,
        conditions,
        actions,
    };
    world.network.add_rule(rule).await.unwrap();
}

#[when("I list all rules")]
async fn when_list_rules(world: &mut World) {
    let rules = world.network.get_all_rules().unwrap();
    world.last_json = serde_json::to_string_pretty(&rules).unwrap();
}

#[when(expr = "I get the rule {string}")]
async fn when_get_rule(world: &mut World, rule_id: String) {
    match world.network.get_rule_by_id(&rule_id).unwrap() {
        Some(rule) => {
            world.last_json = serde_json::to_string_pretty(&rule).unwrap();
        }
        None => {
            world.last_json = format!("Rule '{}' not found", rule_id);
        }
    }
}

#[when(expr = "I remove the rule {string}")]
async fn when_remove_rule(world: &mut World, rule_id: String) {
    world.network.remove_rule(&rule_id).unwrap();
}

#[when(expr = "I assert the fact {string} with predicate {string} and args {string}")]
async fn when_assert_fact(world: &mut World, id: String, predicate: String, args: String) {
    let fact = Fact {
        id,
        predicate,
        args: vec![args],
        timestamp: 0,
    };
    world.network.assert_fact(fact).await.unwrap();
}

#[when(expr = "I retract the fact {string}")]
async fn when_retract_fact(world: &mut World, fact_id: String) {
    let wme = world.network.retract_fact(&fact_id).await.unwrap();
    world.last_json = serde_json::to_string_pretty(&wme.fact).unwrap();
}

#[when(expr = "I list facts with predicate filter {string}")]
async fn when_list_facts_filtered(world: &mut World, predicate: String) {
    let wmes = world.network.get_all_wmes().await.unwrap();
    let facts: Vec<_> = wmes
        .iter()
        .filter(|wme| wme.fact.predicate == predicate)
        .map(|wme| &wme.fact)
        .collect();
    world.last_json = serde_json::to_string_pretty(&facts).unwrap();
}

#[when("I list all facts")]
async fn when_list_all_facts(world: &mut World) {
    let wmes = world.network.get_all_wmes().await.unwrap();
    let facts: Vec<_> = wmes.iter().map(|wme| &wme.fact).collect();
    world.last_json = serde_json::to_string_pretty(&facts).unwrap();
}

#[when(expr = "I get the fact {string}")]
async fn when_get_fact(world: &mut World, fact_id: String) {
    let wmes = world.network.get_all_wmes().await.unwrap();
    match wmes.iter().find(|wme| wme.fact.id == fact_id) {
        Some(wme) => {
            world.last_json = serde_json::to_string_pretty(&wme.fact).unwrap();
        }
        None => {
            world.last_json = format!("Fact '{}' not found", fact_id);
        }
    }
}

#[when("I fire the rules")]
async fn when_fire_rules(world: &mut World) {
    world.network.update_agenda().await.ok();
    let actions = world.network.execute_all_agenda_items().unwrap_or_default();
    world.last_actions.extend(actions.clone());
    world.last_json = serde_json::to_string_pretty(&serde_json::json!({
        "actions_fired": actions.len(),
        "actions": actions,
    }))
    .unwrap();
}

#[when("I peek at the agenda")]
async fn when_get_agenda(world: &mut World) {
    world.network.update_agenda().await.ok();
    let agenda = world.network.agenda.lock().unwrap();
    let items = agenda.get_all_items();
    let summaries: Vec<_> = items
        .iter()
        .map(|item| {
            serde_json::json!({
                "rule_id": item.rule.id,
                "salience": item.salience,
            })
        })
        .collect();
    world.last_json = serde_json::to_string_pretty(&summaries).unwrap();
}

#[when("I check constraints")]
async fn when_check_constraints(world: &mut World) {
    let violations: Vec<_> = world
        .last_actions
        .iter()
        .filter(|a| a.action_type == "constraint_violation")
        .collect();
    if violations.is_empty() {
        world.last_json = "No constraint violations".to_string();
    } else {
        world.last_json = serde_json::to_string_pretty(&violations).unwrap();
    }
}

#[when(expr = "I get consequences filtered by kind {string}")]
async fn when_get_consequences_filtered(world: &mut World, _kind: String) {
    // In the direct-network test, consequences = accumulated actions
    world.last_json = serde_json::to_string_pretty(&world.last_actions).unwrap();
}

#[when(expr = "I set section context for {string} \\/ {string}")]
async fn when_set_section_context(world: &mut World, file: String, section: String) {
    // Direct fact assertion — mirrors what the MCP tool does internally,
    // since BDD tests exercise the ReteNetwork directly rather than the
    // tool layer.
    let fact = Fact {
        id: "__section_context__".to_string(),
        predicate: "markdown_rule".to_string(),
        args: vec![file, section],
        timestamp: 0,
    };
    world.network.assert_fact(fact).await.unwrap();
}

#[when(expr = "a fact asserting new_content_contains {string} with id {string}")]
async fn when_assert_content_fact_with_id(world: &mut World, content: String, id: String) {
    let fact = Fact {
        id,
        predicate: "new_content_contains".to_string(),
        args: vec![content],
        timestamp: 0,
    };
    world.network.assert_fact(fact).await.unwrap();
}

#[when(expr = "I extract rules from the document {string}")]
async fn when_extract_rules(world: &mut World, source: String) {
    world.extracted_rules = extract_rules_from_markdown(&world.markdown_content, &source);
}

#[when("I extract rules from the temp file via the MCP tool")]
async fn when_extract_rules_from_file(world: &mut World) {
    let path = world.temp_file_path.clone().expect("temp file must exist");
    let content = std::fs::read_to_string(&path).unwrap();
    let rules = extract_rules_from_markdown(&content, &path);
    let count = rules.len();
    for rule in &rules {
        world.network.add_rule(rule.clone()).await.unwrap();
    }
    world.extracted_rules = rules;
    world.last_json = format!("Extracted {} rule(s) from '{}'", count, path);
}

#[when(expr = "I run pre-check with tool {string} and input")]
async fn when_run_pre_check(world: &mut World, tool_name: String, step: &cucumber::gherkin::Step) {
    let input_json: serde_json::Value =
        serde_json::from_str(&step.docstring.clone().unwrap_or_default()).unwrap();
    let payload = serde_json::json!({
        "tool_name": tool_name,
        "tool_input": input_json,
    });
    let cwd = world
        .rules_dir
        .as_ref()
        .map(|d| d.path().to_string_lossy().to_string());
    let (code, stderr) = run_hook("pre-check", &payload.to_string(), cwd.as_deref());
    world.last_exit_code = Some(code);
    world.last_stderr = stderr;
}

#[when(expr = "I run post-check with tool {string} and input")]
async fn when_run_post_check(world: &mut World, tool_name: String, step: &cucumber::gherkin::Step) {
    let input_json: serde_json::Value =
        serde_json::from_str(&step.docstring.clone().unwrap_or_default()).unwrap();
    let payload = serde_json::json!({
        "tool_name": tool_name,
        "tool_input": input_json,
    });
    let cwd = world
        .rules_dir
        .as_ref()
        .map(|d| d.path().to_string_lossy().to_string());
    let (code, stderr) = run_hook("post-check", &payload.to_string(), cwd.as_deref());
    world.last_exit_code = Some(code);
    world.last_stderr = stderr;
}

#[when("I run post-check for the checked file")]
async fn when_run_post_check_for_file(world: &mut World) {
    let file_path = world
        .checked_file_path
        .clone()
        .expect("checked_file_path must be set");
    let payload = serde_json::json!({
        "tool_name": "Write",
        "tool_input": { "file_path": file_path, "content": "fn main() {}" },
    });
    let cwd = world
        .rules_dir
        .as_ref()
        .map(|d| d.path().to_string_lossy().to_string());
    let (code, stderr) = run_hook("post-check", &payload.to_string(), cwd.as_deref());
    world.last_exit_code = Some(code);
    world.last_stderr = stderr;
}

// ---------------------------------------------------------------------------
// Then steps
// ---------------------------------------------------------------------------

#[then(expr = "the rule {string} should exist in the network")]
async fn then_rule_exists(world: &mut World, rule_id: String) {
    let rule = world.network.get_rule_by_id(&rule_id).unwrap();
    assert!(
        rule.is_some(),
        "Rule '{}' should exist in the network",
        rule_id
    );
}

#[then(expr = "the network should contain {int} rule(s)")]
async fn then_network_rule_count(world: &mut World, count: usize) {
    let rules = world.network.get_all_rules().unwrap();
    assert_eq!(
        rules.len(),
        count,
        "Expected {} rules, got {}",
        count,
        rules.len()
    );
}

#[then(expr = "I should see {int} rules")]
async fn then_see_n_rules(world: &mut World, count: usize) {
    let rules: Vec<serde_json::Value> = serde_json::from_str(&world.last_json).unwrap_or_default();
    assert_eq!(rules.len(), count);
}

#[then(expr = "the result should contain {string}")]
async fn then_result_contains(world: &mut World, expected: String) {
    assert!(
        world.last_json.contains(&expected),
        "Expected result to contain '{}' but got:\n{}",
        expected,
        world.last_json
    );
}

#[then(expr = "the rule priority should be {int}")]
async fn then_rule_priority(world: &mut World, expected: i32) {
    let rule: serde_json::Value = serde_json::from_str(&world.last_json).unwrap();
    let priority = rule["priority"].as_i64().unwrap() as i32;
    assert_eq!(priority, expected);
}

#[then(expr = "the fact {string} should exist in working memory")]
async fn then_fact_exists(world: &mut World, fact_id: String) {
    let wmes = world.network.get_all_wmes().await.unwrap();
    assert!(
        wmes.iter().any(|wme| wme.fact.id == fact_id),
        "Fact '{}' should exist in working memory",
        fact_id
    );
}

#[then(expr = "working memory should contain {int} fact(s)")]
async fn then_wm_fact_count(world: &mut World, count: usize) {
    let wmes = world.network.get_all_wmes().await.unwrap();
    assert_eq!(
        wmes.len(),
        count,
        "Expected {} facts, got {}",
        count,
        wmes.len()
    );
}

#[then(expr = "I should see {int} facts in the result")]
async fn then_see_n_facts(world: &mut World, count: usize) {
    let facts: Vec<serde_json::Value> = serde_json::from_str(&world.last_json).unwrap_or_default();
    assert_eq!(
        facts.len(),
        count,
        "Expected {} facts, got {}",
        count,
        facts.len()
    );
}

#[then(expr = "the result should show {int} actions fired")]
async fn then_actions_fired(world: &mut World, count: usize) {
    let result: serde_json::Value = serde_json::from_str(&world.last_json).unwrap();
    let fired = result["actions_fired"].as_u64().unwrap() as usize;
    assert_eq!(fired, count);
}

#[then("the consequences should contain at least 1 entry")]
async fn then_consequences_not_empty(world: &mut World) {
    assert!(
        !world.last_actions.is_empty(),
        "Expected at least 1 consequence"
    );
}

#[then("the agenda should not be empty")]
async fn then_agenda_not_empty(world: &mut World) {
    assert!(
        world.last_json != "[]",
        "Expected non-empty agenda but got: {}",
        world.last_json
    );
}

#[then("the consequences result should not be empty")]
async fn then_consequences_result_not_empty(world: &mut World) {
    assert!(
        !world.last_actions.is_empty(),
        "Expected consequences but got empty"
    );
}

// --- Markdown extraction ---

#[then(expr = "{int} rule(s) should be extracted")]
async fn then_n_rules_extracted(world: &mut World, count: usize) {
    assert_eq!(
        world.extracted_rules.len(),
        count,
        "Expected {} extracted rules, got {}",
        count,
        world.extracted_rules.len()
    );
}

#[then(expr = "rule {string} should have action text containing {string}")]
async fn then_rule_action_contains(world: &mut World, rule_id: String, expected: String) {
    let rule = world
        .extracted_rules
        .iter()
        .find(|r| r.id == rule_id)
        .unwrap_or_else(|| panic!("Rule '{}' not found in extracted rules", rule_id));
    let action_text: String = rule
        .actions
        .iter()
        .flat_map(|a| a.params.clone())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        action_text.contains(&expected),
        "Expected action text to contain '{}' but got: {}",
        expected,
        action_text
    );
}

#[then(expr = "rule {string} should exist")]
async fn then_extracted_rule_exists(world: &mut World, rule_id: String) {
    assert!(
        world.extracted_rules.iter().any(|r| r.id == rule_id),
        "Rule '{}' not found in extracted rules",
        rule_id
    );
}

#[then(expr = "the extracted rule should have condition predicate {string}")]
async fn then_extracted_condition_predicate(world: &mut World, predicate: String) {
    assert_eq!(world.extracted_rules[0].conditions[0].predicate, predicate);
}

#[then(expr = "the extracted rule action text contains {string}")]
async fn then_extracted_action_contains(world: &mut World, expected: String) {
    let action_text: String = world.extracted_rules[0]
        .actions
        .iter()
        .flat_map(|a| a.params.clone())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(
        action_text.contains(&expected),
        "Expected action text to contain '{}' but got: {}",
        expected,
        action_text
    );
}

#[then(expr = "the extracted rule should have condition arg {string}")]
async fn then_extracted_condition_arg(world: &mut World, arg: String) {
    assert!(world.extracted_rules[0].conditions[0].args.contains(&arg));
}

#[then(expr = "the MCP result should confirm {int} rules extracted")]
async fn then_mcp_extraction_count(world: &mut World, count: usize) {
    assert!(
        world
            .last_json
            .contains(&format!("Extracted {} rule(s)", count)),
        "Expected extraction confirmation for {} rules but got: {}",
        count,
        world.last_json
    );
}

// --- Hook steps ---

#[then(expr = "the hook should exit with code {int}")]
async fn then_hook_exit_code(world: &mut World, code: i32) {
    assert_eq!(
        world.last_exit_code,
        Some(code),
        "Expected exit code {} but got {:?}",
        code,
        world.last_exit_code
    );
}

#[then(expr = "stderr should contain {string}")]
async fn then_stderr_contains(world: &mut World, expected: String) {
    assert!(
        world.last_stderr.contains(&expected),
        "Expected stderr to contain '{}' but got: {}",
        expected,
        world.last_stderr
    );
}

// ---------------------------------------------------------------------------
// Runner
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() {
    World::run("tests/features").await;
}
