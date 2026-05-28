# Rule Schema v2 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship phronesis-mcp 0.8.0 — a readable `rules.json` wire format (`when`/`then`/predicate-as-key), a first-class `OR` operator expanded at load time, and a `migrate-rules` command, with zero disruption to existing projects.

**Architecture:** All changes live in `crates/phronesis-mcp`. The `phr` library crate is untouched, so library consumers (rulgamr) are unaffected. A new on-disk type `SourceRule` (OR-bearing) is parsed from both v1 and v2 JSON; `unfold_or` expands it into the existing flat `DiskRule` (unchanged shape, so `audit.rs`/`merge`/`rule_from_disk` need no edits). `read()` unfolds; `read_source()` preserves OR for migration; `write_atomic()` emits v2 from flat rules; `write_source()` emits v2 preserving OR.

**Tech Stack:** Rust, serde / serde_json, clap, the existing phronesis-mcp test harness (`cargo test -p phronesis-mcp`).

---

## File Structure

| File | Responsibility | Change |
|------|---------------|--------|
| `crates/phronesis-mcp/src/rules_file.rs` | Disk format types + serde + OR unfold | Add `SourceRule`, `WhenClause`, custom (de)serialize, `unfold_or`, `read_source`, `write_source`; rewrite `read`/`write_atomic` |
| `crates/phronesis-mcp/src/hook.rs` | Pre/post hook rule loading | Refactor `load_rules` to call `rules_file::read` (removes inline parser + dropped-script bug) |
| `crates/phronesis-mcp/src/main.rs` | CLI | Add `migrate-rules` subcommand + handler |
| `crates/phronesis-mcp/src/init.rs` | Default rule packs | Rewrite all default rules in v2 shape |
| `crates/phronesis-mcp/Cargo.toml` | Version | 0.7.0 → 0.8.0 |
| `crates/phronesis-mcp/CLAUDE.md` | Docs | Document v2 shape + migrate-rules |
| Test files (7) | Fixtures | Convert old-shape JSON literals to v2; keep one v1 compat test |

**Type model (locked here):**

```rust
// UNCHANGED — flat, OR-free. Consumed by audit.rs, merge, rule_from_disk.
pub struct DiskRule {
    pub id: String, pub phase: String, pub priority: i32,
    pub conditions: Vec<DiskCondition>,
    pub actions: Vec<DiskAction>,
    pub silent: Option<bool>, pub audit: Option<bool>, pub doc_excepted: Option<bool>,
}
pub struct DiskCondition { pub predicate: String, pub args: Vec<String>, pub script: Option<String> }
pub struct DiskAction { pub action_type: String, pub params: Vec<String> }

// NEW — on-disk v2 form, OR-bearing.
pub struct SourceRule {
    pub id: String, pub phase: String, pub priority: i32,
    pub when: Vec<WhenClause>,
    pub then: DiskAction,
    pub silent: Option<bool>, pub audit: Option<bool>, pub doc_excepted: Option<bool>,
}
pub enum WhenClause { Leaf(DiskCondition), Or(Vec<WhenClause>) }
```

Note: `DiskRule`, `DiskCondition`, `DiskAction` lose their `#[derive(Serialize, Deserialize)]` — serialization moves to `SourceRule`. Keep `#[derive(Debug, Clone)]`. `audit.rs` constructs `DiskRule`/`DiskCondition`/`DiskAction` by struct literal in tests; those keep working because the fields are unchanged.

---

## COMMIT 1 — v2 wire format (serde rewrite)

### Task 1.1: Add `WhenClause` + `SourceRule` types and v2 condition deserialize

**Files:**
- Modify: `crates/phronesis-mcp/src/rules_file.rs`

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` block in `rules_file.rs`:

```rust
#[test]
fn v2_leaf_condition_single_string_arg() {
    let json = r#"{ "new_content_contains": ".unwrap()" }"#;
    let clause: WhenClause = serde_json::from_str(json).unwrap();
    match clause {
        WhenClause::Leaf(c) => {
            assert_eq!(c.predicate, "new_content_contains");
            assert_eq!(c.args, vec![".unwrap()".to_string()]);
            assert_eq!(c.script, None);
        }
        _ => panic!("expected Leaf"),
    }
}

#[test]
fn v2_leaf_condition_multi_arg_array() {
    let json = r#"{ "function_param_count_high": ["?file", "?fn", "?count"] }"#;
    let clause: WhenClause = serde_json::from_str(json).unwrap();
    match clause {
        WhenClause::Leaf(c) => {
            assert_eq!(c.predicate, "function_param_count_high");
            assert_eq!(c.args, vec!["?file".to_string(), "?fn".to_string(), "?count".to_string()]);
        }
        _ => panic!("expected Leaf"),
    }
}

#[test]
fn v2_leaf_condition_zero_arg_bool() {
    let json = r#"{ "some_zero_arg_predicate": true }"#;
    let clause: WhenClause = serde_json::from_str(json).unwrap();
    match clause {
        WhenClause::Leaf(c) => {
            assert_eq!(c.predicate, "some_zero_arg_predicate");
            assert!(c.args.is_empty());
        }
        _ => panic!("expected Leaf"),
    }
}

#[test]
fn v2_script_condition() {
    let json = r#"{ "__script__": "rank > 5" }"#;
    let clause: WhenClause = serde_json::from_str(json).unwrap();
    match clause {
        WhenClause::Leaf(c) => {
            assert_eq!(c.predicate, "__script__");
            assert_eq!(c.script, Some("rank > 5".to_string()));
        }
        _ => panic!("expected Leaf"),
    }
}

#[test]
fn v2_or_clause() {
    let json = r#"{ "or": [ { "new_content_contains": "cargo test" }, { "new_content_contains": "cargo nextest" } ] }"#;
    let clause: WhenClause = serde_json::from_str(json).unwrap();
    match clause {
        WhenClause::Or(alts) => assert_eq!(alts.len(), 2),
        _ => panic!("expected Or"),
    }
}
```

- [ ] **Step 2: Run, verify it fails to compile (WhenClause undefined)**

Run: `cargo test -p phronesis-mcp --lib rules_file::tests::v2_ 2>&1 | head -20`
Expected: compile error, `cannot find type WhenClause`.

- [ ] **Step 3: Add the types + custom Deserialize for `WhenClause`**

Add near the top of `rules_file.rs`, after the existing `use` lines:

```rust
use serde::de::{self, MapAccess, Visitor};
use std::fmt;

/// One clause in a v2 rule's `when` array: a leaf condition or an OR group.
#[derive(Debug, Clone)]
pub enum WhenClause {
    Leaf(DiskCondition),
    Or(Vec<WhenClause>),
}

impl<'de> Deserialize<'de> for WhenClause {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        // A clause is always a single-key JSON object. If the key is "or",
        // it's a disjunction; otherwise the key is a predicate name.
        let value = serde_json::Value::deserialize(deserializer)?;
        let obj = value
            .as_object()
            .ok_or_else(|| de::Error::custom("when-clause must be a JSON object"))?;
        if obj.len() != 1 {
            return Err(de::Error::custom(
                "when-clause must have exactly one key (a predicate name or \"or\")",
            ));
        }
        let (key, val) = obj.iter().next().expect("len checked == 1");

        if key == "or" {
            let arr = val
                .as_array()
                .ok_or_else(|| de::Error::custom("\"or\" value must be an array of clauses"))?;
            let alts: Result<Vec<WhenClause>, _> = arr
                .iter()
                .map(|v| serde_json::from_value::<WhenClause>(v.clone()).map_err(de::Error::custom))
                .collect();
            return Ok(WhenClause::Or(alts?));
        }

        // Leaf condition. Key is the predicate name.
        let predicate = key.clone();
        let (args, script) = if predicate == "__script__" {
            let s = val
                .as_str()
                .ok_or_else(|| de::Error::custom("__script__ value must be a string"))?;
            (Vec::new(), Some(s.to_string()))
        } else {
            let args = match val {
                serde_json::Value::String(s) => vec![s.clone()],
                serde_json::Value::Bool(_) => Vec::new(),
                serde_json::Value::Array(items) => {
                    let mut out = Vec::with_capacity(items.len());
                    for it in items {
                        let s = it.as_str().ok_or_else(|| {
                            de::Error::custom("predicate arg array must contain strings")
                        })?;
                        out.push(s.to_string());
                    }
                    out
                }
                other => {
                    return Err(de::Error::custom(format!(
                        "predicate value must be string, array, or bool; got {}",
                        other
                    )))
                }
            };
            (args, None)
        };
        Ok(WhenClause::Leaf(DiskCondition {
            predicate,
            args,
            script,
        }))
    }
}
```

Also: remove `Serialize, Deserialize` from the `#[derive(...)]` on `DiskCondition`, `DiskAction`, `DiskRule`, **and `RulesFile`** (leave `Debug, Clone`). They'll be (de)serialized only via `SourceRule` from now on. This is safe: the only direct `serde_json::from_str::<RulesFile>` call is in the old `read` body, which Task 1.4 replaces, and the only direct serialization is the old `write_atomic` body, also replaced. Nothing else (de)serializes these types directly — verified by grep. The `de` import is included above.

- [ ] **Step 4: Run, verify the 5 tests pass**

Run: `cargo test -p phronesis-mcp --lib rules_file::tests::v2_ 2>&1 | tail -10`
Expected: 5 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/phronesis-mcp/src/rules_file.rs
git commit -m "feat(rules): v2 when-clause deserialize (leaf + or)"
```

---

### Task 1.2: v2 action verb deserialize + `SourceRule` deserialize (both shapes)

**Files:**
- Modify: `crates/phronesis-mcp/src/rules_file.rs`

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn v2_action_block() {
    let json = r#"{ "block": "no unwrap" }"#;
    let a = parse_then_action(&serde_json::from_str(json).unwrap()).unwrap();
    assert_eq!(a.action_type, "constraint_violation");
    assert_eq!(a.params, vec!["no unwrap".to_string()]);
}

#[test]
fn v2_action_warn_and_log() {
    let w = parse_then_action(&serde_json::from_str(r#"{ "warn": "m" }"#).unwrap()).unwrap();
    assert_eq!(w.action_type, "constraint_warning");
    let l = parse_then_action(&serde_json::from_str(r#"{ "log": "m" }"#).unwrap()).unwrap();
    assert_eq!(l.action_type, "log");
}

#[test]
fn source_rule_parses_v2() {
    let json = r#"{
        "id": "r1", "phase": "pre", "priority": 10, "audit": true,
        "when": [ { "new_content_contains": ".unwrap()" }, { "file_path_matches": "src" } ],
        "then": { "block": "no unwrap" }
    }"#;
    let sr: SourceRule = serde_json::from_str(json).unwrap();
    assert_eq!(sr.id, "r1");
    assert_eq!(sr.when.len(), 2);
    assert_eq!(sr.then.action_type, "constraint_violation");
    assert_eq!(sr.audit, Some(true));
}

#[test]
fn source_rule_parses_v1_legacy() {
    let json = r#"{
        "id": "r1", "phase": "pre", "priority": 10,
        "conditions": [ {"predicate":"new_content_contains","args":[".unwrap()"]} ],
        "actions": [ {"action_type":"constraint_violation","params":["no unwrap"]} ]
    }"#;
    let sr: SourceRule = serde_json::from_str(json).unwrap();
    assert_eq!(sr.id, "r1");
    assert_eq!(sr.when.len(), 1);
    match &sr.when[0] {
        WhenClause::Leaf(c) => assert_eq!(c.predicate, "new_content_contains"),
        _ => panic!("expected leaf"),
    }
    assert_eq!(sr.then.action_type, "constraint_violation");
}
```

- [ ] **Step 2: Run, verify it fails to compile**

Run: `cargo test -p phronesis-mcp --lib rules_file::tests::source_rule 2>&1 | head -20`
Expected: compile error — `SourceRule`, `parse_then_action` undefined.

- [ ] **Step 3: Implement `SourceRule`, `parse_then_action`, action-verb mapping, and the dual-shape Deserialize**

```rust
/// On-disk v2 rule form. OR-bearing; expanded to flat `DiskRule`s by `unfold_or`.
#[derive(Debug, Clone)]
pub struct SourceRule {
    pub id: String,
    pub phase: String,
    pub priority: i32,
    pub when: Vec<WhenClause>,
    pub then: DiskAction,
    pub silent: Option<bool>,
    pub audit: Option<bool>,
    pub doc_excepted: Option<bool>,
}

/// Map a v2 `then` object (`{"block": "msg"}`) to an internal action.
/// `block`→constraint_violation, `warn`→constraint_warning, `log`→log,
/// anything else passes through as its own action_type (forward-compat).
fn parse_then_action(value: &serde_json::Value) -> Result<DiskAction, String> {
    let obj = value
        .as_object()
        .ok_or_else(|| "then must be a JSON object".to_string())?;
    if obj.len() != 1 {
        return Err("then must have exactly one verb key".to_string());
    }
    let (verb, msg_val) = obj.iter().next().expect("len==1");
    let msg = msg_val
        .as_str()
        .ok_or_else(|| "then message must be a string".to_string())?
        .to_string();
    let action_type = match verb.as_str() {
        "block" => "constraint_violation",
        "warn" => "constraint_warning",
        "log" => "log",
        other => other,
    }
    .to_string();
    Ok(DiskAction {
        action_type,
        params: vec![msg],
    })
}

/// Inverse of `parse_then_action`: internal action → v2 verb object.
fn action_to_then(action: &DiskAction) -> serde_json::Value {
    let verb = match action.action_type.as_str() {
        "constraint_violation" => "block",
        "constraint_warning" => "warn",
        "log" => "log",
        other => other,
    };
    let msg = action.params.first().cloned().unwrap_or_default();
    serde_json::json!({ verb: msg })
}

impl<'de> Deserialize<'de> for SourceRule {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let v = serde_json::Value::deserialize(deserializer)?;
        let obj = v
            .as_object()
            .ok_or_else(|| de::Error::custom("rule must be a JSON object"))?;

        let id = obj
            .get("id")
            .and_then(|x| x.as_str())
            .ok_or_else(|| de::Error::custom("rule missing string `id`"))?
            .to_string();
        let phase = obj
            .get("phase")
            .and_then(|x| x.as_str())
            .unwrap_or("pre")
            .to_string();
        let priority = obj.get("priority").and_then(|x| x.as_i64()).unwrap_or(0) as i32;
        let silent = obj.get("silent").and_then(|x| x.as_bool());
        let audit = obj.get("audit").and_then(|x| x.as_bool());
        let doc_excepted = obj.get("doc_excepted").and_then(|x| x.as_bool());

        // Conditions: v2 `when` takes precedence; fall back to v1 `conditions`.
        let when: Vec<WhenClause> = if let Some(when_val) = obj.get("when") {
            let arr = when_val
                .as_array()
                .ok_or_else(|| de::Error::custom("`when` must be an array"))?;
            arr.iter()
                .map(|c| serde_json::from_value::<WhenClause>(c.clone()).map_err(de::Error::custom))
                .collect::<Result<_, _>>()?
        } else if let Some(cond_val) = obj.get("conditions") {
            // v1 legacy: each is {predicate, args, script?}.
            let arr = cond_val
                .as_array()
                .ok_or_else(|| de::Error::custom("`conditions` must be an array"))?;
            arr.iter()
                .map(|c| {
                    let co = c
                        .as_object()
                        .ok_or_else(|| de::Error::custom("v1 condition must be an object"))?;
                    let predicate = co
                        .get("predicate")
                        .and_then(|x| x.as_str())
                        .ok_or_else(|| de::Error::custom("v1 condition missing `predicate`"))?
                        .to_string();
                    let args = co
                        .get("args")
                        .and_then(|x| x.as_array())
                        .map(|a| {
                            a.iter()
                                .filter_map(|s| s.as_str().map(String::from))
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default();
                    let script = co.get("script").and_then(|x| x.as_str()).map(String::from);
                    Ok(WhenClause::Leaf(DiskCondition {
                        predicate,
                        args,
                        script,
                    }))
                })
                .collect::<Result<_, _>>()?
        } else {
            return Err(de::Error::custom("rule has neither `when` nor `conditions`"));
        };

        // Action: v2 `then` takes precedence; fall back to v1 `actions[0]`.
        let then: DiskAction = if let Some(then_val) = obj.get("then") {
            parse_then_action(then_val).map_err(de::Error::custom)?
        } else if let Some(actions_val) = obj.get("actions") {
            let first = actions_val
                .as_array()
                .and_then(|a| a.first())
                .ok_or_else(|| de::Error::custom("v1 `actions` must be a non-empty array"))?;
            let ao = first
                .as_object()
                .ok_or_else(|| de::Error::custom("v1 action must be an object"))?;
            let action_type = ao
                .get("action_type")
                .and_then(|x| x.as_str())
                .ok_or_else(|| de::Error::custom("v1 action missing `action_type`"))?
                .to_string();
            let params = ao
                .get("params")
                .and_then(|x| x.as_array())
                .map(|a| a.iter().filter_map(|s| s.as_str().map(String::from)).collect())
                .unwrap_or_default();
            DiskAction {
                action_type,
                params,
            }
        } else {
            return Err(de::Error::custom("rule has neither `then` nor `actions`"));
        };

        Ok(SourceRule {
            id,
            phase,
            priority,
            when,
            then,
            silent,
            audit,
            doc_excepted,
        })
    }
}
```

- [ ] **Step 4: Run, verify the 4 tests pass**

Run: `cargo test -p phronesis-mcp --lib rules_file::tests 2>&1 | tail -10`
Expected: all rules_file tests pass (the v2 leaf tests from 1.1 plus these 4).

- [ ] **Step 5: Commit**

```bash
git add crates/phronesis-mcp/src/rules_file.rs
git commit -m "feat(rules): SourceRule deserialize accepts v1 and v2 shapes"
```

---

### Task 1.3: v2 Serialize for `SourceRule` + `RulesFile` write path

**Files:**
- Modify: `crates/phronesis-mcp/src/rules_file.rs`

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn source_rule_serializes_v2_round_trip() {
    let json = r#"{
        "id": "r1", "phase": "pre", "priority": 10, "audit": true,
        "when": [ { "new_content_contains": ".unwrap()" }, { "file_path_matches": "src" } ],
        "then": { "block": "no unwrap" }
    }"#;
    let sr: SourceRule = serde_json::from_str(json).unwrap();
    let out = serde_json::to_value(&sr).unwrap();
    // Re-parse the serialized form; must be identical SourceRule.
    let sr2: SourceRule = serde_json::from_value(out.clone()).unwrap();
    assert_eq!(sr2.id, "r1");
    assert_eq!(sr2.when.len(), 2);
    assert_eq!(sr2.then.action_type, "constraint_violation");
    // Spot-check the emitted shape is v2, not v1.
    assert!(out.get("when").is_some());
    assert!(out.get("conditions").is_none());
    assert_eq!(out["then"]["block"], "no unwrap");
    assert_eq!(out["when"][0]["new_content_contains"], ".unwrap()");
}

#[test]
fn source_rule_serializes_or_clause() {
    let json = r#"{
        "id": "r1", "phase": "pre", "priority": 5,
        "when": [ { "or": [ { "new_content_contains": "a" }, { "new_content_contains": "b" } ] } ],
        "then": { "warn": "m" }
    }"#;
    let sr: SourceRule = serde_json::from_str(json).unwrap();
    let out = serde_json::to_value(&sr).unwrap();
    assert!(out["when"][0]["or"].is_array());
    assert_eq!(out["when"][0]["or"][0]["new_content_contains"], "a");
}
```

- [ ] **Step 2: Run, verify failure**

Run: `cargo test -p phronesis-mcp --lib rules_file::tests::source_rule_serializes 2>&1 | head -20`
Expected: compile error — `SourceRule` does not implement `Serialize`.

- [ ] **Step 3: Implement Serialize for `WhenClause` and `SourceRule`**

```rust
impl Serialize for WhenClause {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let v = match self {
            WhenClause::Or(alts) => {
                let arr: Vec<serde_json::Value> = alts
                    .iter()
                    .map(|c| serde_json::to_value(c).unwrap_or(serde_json::Value::Null))
                    .collect();
                serde_json::json!({ "or": arr })
            }
            WhenClause::Leaf(c) => {
                if let Some(script) = &c.script {
                    serde_json::json!({ "__script__": script })
                } else {
                    let value = match c.args.len() {
                        0 => serde_json::Value::Bool(true),
                        1 => serde_json::Value::String(c.args[0].clone()),
                        _ => serde_json::Value::Array(
                            c.args.iter().cloned().map(serde_json::Value::String).collect(),
                        ),
                    };
                    serde_json::json!({ c.predicate.clone(): value })
                }
            }
        };
        v.serialize(serializer)
    }
}

impl Serialize for SourceRule {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap;
        // Pinned key order: id, phase, priority, [audit, silent, doc_excepted], when, then.
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("id", &self.id)?;
        map.serialize_entry("phase", &self.phase)?;
        map.serialize_entry("priority", &self.priority)?;
        if let Some(a) = self.audit {
            map.serialize_entry("audit", &a)?;
        }
        if let Some(s) = self.silent {
            map.serialize_entry("silent", &s)?;
        }
        if let Some(d) = self.doc_excepted {
            map.serialize_entry("doc_excepted", &d)?;
        }
        map.serialize_entry("when", &self.when)?;
        map.serialize_entry("then", &action_to_then(&self.then))?;
        map.end()
    }
}
```

- [ ] **Step 4: Run, verify pass**

Run: `cargo test -p phronesis-mcp --lib rules_file::tests::source_rule_serializes 2>&1 | tail -10`
Expected: 2 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/phronesis-mcp/src/rules_file.rs
git commit -m "feat(rules): v2 serialize for SourceRule (when/then/predicate-as-key)"
```

---

### Task 1.4: Rewire `read` / `write_atomic` through `SourceRule`; add `read_source` / `write_source`

**Files:**
- Modify: `crates/phronesis-mcp/src/rules_file.rs`

This task introduces `unfold_or` as a stub that errors on OR (real expansion lands in Commit 2). That keeps Commit 1 self-contained: v1/v2 non-OR files work end-to-end; an OR file errors clearly until Commit 2.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn read_parses_v2_file_to_flat_diskrules() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rules.json");
    std::fs::write(&path, r#"{ "rules": [
        { "id": "r1", "phase": "pre", "priority": 10,
          "when": [ { "new_content_contains": ".unwrap()" } ],
          "then": { "block": "no unwrap" } }
    ] }"#).unwrap();
    let file = read(&path).unwrap();
    assert_eq!(file.rules.len(), 1);
    assert_eq!(file.rules[0].conditions[0].predicate, "new_content_contains");
    assert_eq!(file.rules[0].actions[0].action_type, "constraint_violation");
}

#[test]
fn read_parses_v1_legacy_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rules.json");
    std::fs::write(&path, r#"{ "rules": [
        { "id": "r1", "phase": "pre", "priority": 10,
          "conditions": [ {"predicate":"new_content_contains","args":[".unwrap()"]} ],
          "actions": [ {"action_type":"constraint_violation","params":["no unwrap"]} ] }
    ] }"#).unwrap();
    let file = read(&path).unwrap();
    assert_eq!(file.rules.len(), 1);
    assert_eq!(file.rules[0].conditions[0].predicate, "new_content_contains");
}

#[test]
fn write_atomic_emits_v2() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rules.json");
    let rf = RulesFile {
        rules: vec![DiskRule {
            id: "r1".into(), phase: "pre".into(), priority: 10,
            conditions: vec![DiskCondition { predicate: "new_content_contains".into(), args: vec![".unwrap()".into()], script: None }],
            actions: vec![DiskAction { action_type: "constraint_violation".into(), params: vec!["no unwrap".into()] }],
            silent: None, audit: Some(true), doc_excepted: None,
        }],
    };
    write_atomic(&path, &rf).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("\"when\""));
    assert!(text.contains("\"then\""));
    assert!(text.contains("\"block\""));
    assert!(!text.contains("\"action_type\""));
}
```

- [ ] **Step 2: Run, verify failure**

Run: `cargo test -p phronesis-mcp --lib rules_file::tests::read_parses 2>&1 | head -20`
Expected: failures — `read` still uses derive-based parsing that no longer exists / returns wrong shape.

- [ ] **Step 3: Rewrite `read`, `write_atomic`; add helpers + `unfold_or` stub**

First, add a new variant to `RulesFileError` (the enum already uses `thiserror`):

```rust
    #[error("rules file at {path} could not be expanded: {message}")]
    Unfold { path: String, message: String },
```

Replace the body of `read`:

```rust
pub fn read(path: &Path) -> Result<RulesFile, RulesFileError> {
    let sources = read_source(path)?;
    let mut flat = Vec::new();
    for sr in &sources {
        let expanded = unfold_or(sr).map_err(|message| RulesFileError::Unfold {
            path: path.display().to_string(),
            message,
        })?;
        flat.extend(expanded);
    }
    Ok(RulesFile { rules: flat })
}

/// Parse the file into OR-bearing `SourceRule`s without unfolding. Used by
/// migration (which preserves OR on disk).
pub fn read_source(path: &Path) -> Result<Vec<SourceRule>, RulesFileError> {
    if !path.exists() {
        return Ok(vec![]);
    }
    let content = std::fs::read_to_string(path).map_err(|e| RulesFileError::Io {
        path: path.display().to_string(),
        source: e,
    })?;
    #[derive(Deserialize)]
    struct Wrapper {
        rules: Vec<SourceRule>,
    }
    let w: Wrapper = serde_json::from_str(&content).map_err(|e| RulesFileError::Malformed {
        path: path.display().to_string(),
        source: e,
    })?;
    Ok(w.rules)
}
```

Add the stub for `unfold_or` (real impl in Commit 2):

```rust
/// Expand a SourceRule's OR clauses into flat, OR-free DiskRules.
/// COMMIT 1 STUB: errors if any OR is present. Real expansion lands in Commit 2.
pub fn unfold_or(source: &SourceRule) -> Result<Vec<DiskRule>, String> {
    let mut conditions = Vec::new();
    for clause in &source.when {
        match clause {
            WhenClause::Leaf(c) => conditions.push(c.clone()),
            WhenClause::Or(_) => {
                return Err(format!(
                    "rule `{}` uses `or`, not yet supported (Commit 2)",
                    source.id
                ))
            }
        }
    }
    Ok(vec![DiskRule {
        id: source.id.clone(),
        phase: source.phase.clone(),
        priority: source.priority,
        conditions,
        actions: vec![source.then.clone()],
        silent: source.silent,
        audit: source.audit,
        doc_excepted: source.doc_excepted,
    }])
}
```

Rewrite `write_atomic` to emit v2 by converting flat `DiskRule`s to `SourceRule`s:

```rust
pub fn write_atomic(path: &Path, file: &RulesFile) -> Result<(), RulesFileError> {
    let sources: Vec<SourceRule> = file.rules.iter().map(diskrule_to_source).collect();
    write_source(path, &sources)
}

/// Write OR-bearing SourceRules to disk in v2 shape. Used by migration.
pub fn write_source(path: &Path, sources: &[SourceRule]) -> Result<(), RulesFileError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| RulesFileError::Io {
            path: parent.display().to_string(),
            source: e,
        })?;
    }
    if path.exists() {
        let bak = path.with_extension("json.bak");
        std::fs::copy(path, &bak).map_err(|e| RulesFileError::Io {
            path: bak.display().to_string(),
            source: e,
        })?;
    }
    #[derive(Serialize)]
    struct Wrapper<'a> {
        rules: &'a [SourceRule],
    }
    let json = serde_json::to_string_pretty(&Wrapper { rules: sources })?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json).map_err(|e| RulesFileError::Io {
        path: tmp.display().to_string(),
        source: e,
    })?;
    std::fs::rename(&tmp, path).map_err(|e| RulesFileError::Io {
        path: path.display().to_string(),
        source: e,
    })?;
    Ok(())
}

/// Flat DiskRule → SourceRule (all-Leaf when, single then). Inverse of the
/// no-OR case of unfold_or; used by the writer.
fn diskrule_to_source(d: &DiskRule) -> SourceRule {
    SourceRule {
        id: d.id.clone(),
        phase: d.phase.clone(),
        priority: d.priority,
        when: d.conditions.iter().cloned().map(WhenClause::Leaf).collect(),
        then: d.actions.first().cloned().unwrap_or(DiskAction {
            action_type: "log".to_string(),
            params: vec![String::new()],
        }),
        silent: d.silent,
        audit: d.audit,
        doc_excepted: d.doc_excepted,
    }
}
```

- [ ] **Step 4: Run the full rules_file test module**

Run: `cargo test -p phronesis-mcp --lib rules_file 2>&1 | tail -15`
Expected: all pass, including the pre-existing `round_trip_rule` and `merge_*` tests (they use `rule_to_disk`/`merge` which are unchanged).

- [ ] **Step 5: Commit**

```bash
git add crates/phronesis-mcp/src/rules_file.rs
git commit -m "feat(rules): read/write through SourceRule; v2 on disk, both shapes parsed"
```

---

### Task 1.5: Refactor `hook::load_rules` to use `read` (removes inline parser + dropped-script bug)

**Files:**
- Modify: `crates/phronesis-mcp/src/hook.rs:493-541`

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` in `hook.rs` (or the nearest integration point). This test asserts that a script condition survives loading — the current inline parser drops it (`script: None`):

```rust
#[test]
fn load_rules_preserves_script_condition() {
    use crate::rules_file::{self, DiskAction, DiskCondition, DiskRule, RulesFile};
    let dir = tempfile::tempdir().unwrap();
    let phr = dir.path().join(".phronesis");
    std::fs::create_dir_all(&phr).unwrap();
    let rf = RulesFile { rules: vec![DiskRule {
        id: "s".into(), phase: "pre".into(), priority: 1,
        conditions: vec![DiskCondition { predicate: "__script__".into(), args: vec![], script: Some("rank > 5".into()) }],
        actions: vec![DiskAction { action_type: "constraint_warning".into(), params: vec!["m".into()] }],
        silent: None, audit: None, doc_excepted: None,
    }]};
    rules_file::write_atomic(&phr.join("rules.json"), &rf).unwrap();

    let loaded = rules_file::read(&phr.join("rules.json")).unwrap();
    assert_eq!(loaded.rules[0].conditions[0].script, Some("rank > 5".to_string()));
}
```

(Placed in `rules_file::tests` is fine — the point is the round-trip preserves script. If you prefer it next to the hook, mirror it there.)

- [ ] **Step 2: Run, verify pass already (read preserves script) OR fail (if a bug remains)**

Run: `cargo test -p phronesis-mcp --lib load_rules_preserves_script 2>&1 | tail -10`
Expected: PASS — confirms `read` preserves script. (This guards the refactor in step 3.)

- [ ] **Step 3: Replace `hook::load_rules` body with a call to `read`**

First add a string-bearing variant to `RulesLoadError` in `hook.rs` (it uses `thiserror`):

```rust
    #[error("rules file at {path} could not be loaded: {message}")]
    Load { path: String, message: String },
```

Replace lines 493-541 (the whole `fn load_rules`) with:

```rust
fn load_rules(phase: &str) -> Result<Option<Vec<Rule>>, RulesLoadError> {
    let path_buf = crate::rules_file::default_path(&security::project_root());
    if !path_buf.exists() {
        return Ok(None);
    }
    let rules_file = crate::rules_file::read(&path_buf).map_err(|e| RulesLoadError::Load {
        path: path_buf.display().to_string(),
        message: e.to_string(),
    })?;

    let rules: Vec<Rule> = rules_file
        .rules
        .into_iter()
        .filter(|r| r.phase == phase)
        .map(|r| crate::rules_file::rule_from_disk(&r).0)
        .collect();

    if rules.is_empty() {
        Ok(None)
    } else {
        Ok(Some(rules))
    }
}
```

If the import of `RulesFile` (and now-unused `Condition`/`Action`/`serde_json` symbols) in `hook.rs` is no longer referenced, remove it to avoid unused-import warnings.

- [ ] **Step 4: Run hook integration tests**

Run: `cargo test -p phronesis-mcp --test hook_integration 2>&1 | tail -15`
Expected: all pass (these still use v1-shape fixtures, which `read` parses via the compat path — proving backward compatibility end-to-end through the hook).

- [ ] **Step 5: Commit**

```bash
git add crates/phronesis-mcp/src/hook.rs crates/phronesis-mcp/src/rules_file.rs
git commit -m "refactor(hook): load_rules uses rules_file::read (dedupes parser, fixes dropped script)"
```

---

## COMMIT 2 — OR operator via load-time unfolding

### Task 2.1: Real `unfold_or` (DNF expansion) + unit tests

**Files:**
- Modify: `crates/phronesis-mcp/src/rules_file.rs`

- [ ] **Step 1: Write the failing tests**

```rust
fn leaf(pred: &str, arg: &str) -> WhenClause {
    WhenClause::Leaf(DiskCondition { predicate: pred.into(), args: vec![arg.into()], script: None })
}
fn src(id: &str, when: Vec<WhenClause>) -> SourceRule {
    SourceRule { id: id.into(), phase: "pre".into(), priority: 1, when,
        then: DiskAction { action_type: "log".into(), params: vec!["m".into()] },
        silent: None, audit: None, doc_excepted: None }
}

#[test]
fn unfold_no_or_passthrough() {
    let s = src("r", vec![leaf("a", "1"), leaf("b", "2")]);
    let out = unfold_or(&s).unwrap();
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].id, "r");
    assert_eq!(out[0].conditions.len(), 2);
}

#[test]
fn unfold_single_or_two_alternatives() {
    let s = src("r", vec![WhenClause::Or(vec![leaf("a", "1"), leaf("b", "2")]), leaf("c", "3")]);
    let out = unfold_or(&s).unwrap();
    assert_eq!(out.len(), 2);
    assert_eq!(out[0].id, "r#or0");
    assert_eq!(out[1].id, "r#or1");
    // Each carries the non-OR leaf c.
    assert!(out[0].conditions.iter().any(|c| c.predicate == "c"));
    assert!(out[0].conditions.iter().any(|c| c.predicate == "a"));
    assert!(out[1].conditions.iter().any(|c| c.predicate == "b"));
}

#[test]
fn unfold_multi_or_cartesian() {
    let s = src("r", vec![
        WhenClause::Or(vec![leaf("a", "1"), leaf("b", "2")]),
        WhenClause::Or(vec![leaf("c", "3"), leaf("d", "4")]),
    ]);
    let out = unfold_or(&s).unwrap();
    assert_eq!(out.len(), 4);
    let ids: Vec<&str> = out.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(ids, vec!["r#or0-or0", "r#or0-or1", "r#or1-or0", "r#or1-or1"]);
}

#[test]
fn unfold_nested_or_flattens() {
    let s = src("r", vec![WhenClause::Or(vec![
        leaf("a", "1"),
        WhenClause::Or(vec![leaf("b", "2"), leaf("c", "3")]),
    ])]);
    let out = unfold_or(&s).unwrap();
    assert_eq!(out.len(), 3); // a, b, c
}

#[test]
fn unfold_empty_or_errors() {
    let s = src("r", vec![WhenClause::Or(vec![])]);
    assert!(unfold_or(&s).is_err());
}

#[test]
fn unfold_single_element_or_degenerates() {
    let s = src("r", vec![WhenClause::Or(vec![leaf("a", "1")])]);
    let out = unfold_or(&s).unwrap();
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].id, "r"); // no suffix when only one product
}
```

- [ ] **Step 2: Run, verify failure**

Run: `cargo test -p phronesis-mcp --lib rules_file::tests::unfold_ 2>&1 | tail -20`
Expected: failures — the stub errors on any OR; multi/nested/cartesian unimplemented.

- [ ] **Step 3: Replace the `unfold_or` stub with the real DNF expansion**

```rust
pub fn unfold_or(source: &SourceRule) -> Result<Vec<DiskRule>, String> {
    // For each `when` position, compute the list of (alt_index, conditions)
    // alternatives. A leaf has exactly one alternative (index ignored). An OR
    // contributes one alternative per flattened branch.
    //
    // Returns (Vec of alternatives) per position, where each alternative is
    // (Option<usize> alt-index-for-id, Vec<DiskCondition>).
    fn alternatives(clause: &WhenClause, rule_id: &str) -> Result<Vec<Vec<DiskCondition>>, String> {
        match clause {
            WhenClause::Leaf(c) => Ok(vec![vec![c.clone()]]),
            WhenClause::Or(alts) => {
                if alts.is_empty() {
                    return Err(format!("rule `{}` has an empty `or` (unsatisfiable)", rule_id));
                }
                let mut out = Vec::new();
                for alt in alts {
                    // Flatten nested OR by recursion: each branch yields its own
                    // alternative(s).
                    let branch = alternatives(alt, rule_id)?;
                    out.extend(branch);
                }
                Ok(out)
            }
        }
    }

    // Per-position alternative sets. Positions that are leaves have a single
    // alternative; OR positions have N. Cartesian product across positions.
    let mut position_alts: Vec<Vec<Vec<DiskCondition>>> = Vec::new();
    let mut is_or_position: Vec<bool> = Vec::new();
    for clause in &source.when {
        let alts = alternatives(clause, &source.id)?;
        is_or_position.push(matches!(clause, WhenClause::Or(_)) && alts.len() > 1);
        position_alts.push(alts);
    }

    // Cartesian product. Track the chosen alt index at each OR position for id.
    let mut results: Vec<(Vec<usize>, Vec<DiskCondition>)> = vec![(Vec::new(), Vec::new())];
    for (pos, alts) in position_alts.iter().enumerate() {
        let mut next = Vec::new();
        for (idx_path, conds) in &results {
            for (alt_idx, alt_conds) in alts.iter().enumerate() {
                let mut new_path = idx_path.clone();
                if is_or_position[pos] {
                    new_path.push(alt_idx);
                }
                let mut new_conds = conds.clone();
                new_conds.extend(alt_conds.clone());
                next.push((new_path, new_conds));
            }
        }
        results = next;
    }

    if results.len() > 32 {
        eprintln!(
            "phronesis: rule `{}` expands to {} rules via OR — consider simplifying",
            source.id,
            results.len()
        );
    }

    let multiple = results.len() > 1;
    Ok(results
        .into_iter()
        .map(|(idx_path, conditions)| {
            let id = if multiple && !idx_path.is_empty() {
                let suffix = idx_path
                    .iter()
                    .map(|i| format!("or{}", i))
                    .collect::<Vec<_>>()
                    .join("-");
                format!("{}#{}", source.id, suffix)
            } else {
                source.id.clone()
            };
            DiskRule {
                id,
                phase: source.phase.clone(),
                priority: source.priority,
                conditions,
                actions: vec![source.then.clone()],
                silent: source.silent,
                audit: source.audit,
                doc_excepted: source.doc_excepted,
            }
        })
        .collect())
}
```

- [ ] **Step 4: Run unfold tests**

Run: `cargo test -p phronesis-mcp --lib rules_file::tests::unfold_ 2>&1 | tail -15`
Expected: all 6 pass.

- [ ] **Step 5: Commit**

```bash
git add crates/phronesis-mcp/src/rules_file.rs
git commit -m "feat(rules): unfold_or — DNF expansion with deterministic #orN ids"
```

---

### Task 2.2: End-to-end OR integration test through a real pre-check

**Files:**
- Create test in: `crates/phronesis-mcp/tests/hook_integration.rs` (append)

- [ ] **Step 1: Write the failing test**

Find the existing helper in `hook_integration.rs` that runs a pre-check against a temp project (e.g. `run_pre_check_in` or similar — match the file's existing harness). Append:

```rust
#[test]
fn or_rule_fires_on_either_branch() {
    let dir = tempfile::tempdir().unwrap();
    let phr = dir.path().join(".phronesis");
    std::fs::create_dir_all(&phr).unwrap();
    std::fs::write(phr.join("rules.json"), r#"{ "rules": [
        { "id": "block-test-cmd", "phase": "pre", "priority": 5,
          "when": [ { "or": [
              { "new_content_contains": "cargo test" },
              { "new_content_contains": "cargo nextest" }
          ] } ],
          "then": { "block": "use the workspace test runner" } }
    ] }"#).unwrap();

    // Payload matching ONLY the second branch (nextest).
    let payload = serde_json::json!({
        "tool_name": "Bash",
        "tool_input": { "command": "cargo nextest run" }
    });
    let (code, stderr) = run_pre_check_with_payload(dir.path(), &payload.to_string());
    assert_eq!(code, 2, "expected block; stderr: {stderr}");
    assert!(stderr.contains("workspace test runner"));
}
```

If the file's harness helper has a different name/signature, adapt this call to it. The assertion that matters: a payload matching only the *second* OR branch still triggers the block — proving unfold→engine.

- [ ] **Step 2: Run, verify it fails**

Run: `cargo test -p phronesis-mcp --test hook_integration or_rule_fires 2>&1 | tail -15`
Expected: PASS once 2.1 is in (unfold produces `block-test-cmd#or0` and `#or1`, and the nextest payload matches `#or1`). If it FAILS, the unfold isn't wired into the hook load path — but it is, because `load_rules` now calls `read`, which unfolds. This test confirms that wiring end to end.

- [ ] **Step 3: (only if step 2 fails) confirm `read` is on the hook path**

No code change expected. If the test fails, verify Task 1.5 landed (load_rules calls read). Fix wiring if needed.

- [ ] **Step 4: Run again, verify pass**

Run: `cargo test -p phronesis-mcp --test hook_integration or_rule_fires 2>&1 | tail -10`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/phronesis-mcp/tests/hook_integration.rs
git commit -m "test(hook): OR rule fires on either branch end-to-end"
```

---

## COMMIT 3 — `phr-mcp migrate-rules`

### Task 3.1: `migrate-rules` subcommand + handler

**Files:**
- Modify: `crates/phronesis-mcp/src/main.rs` (Command enum + match arm)

- [ ] **Step 1: Write the failing test**

Add an integration test file `crates/phronesis-mcp/tests/migrate_integration.rs`:

```rust
use std::process::Command;

fn run_migrate(args: &[&str]) -> std::process::Output {
    let bin = env!("CARGO_BIN_EXE_phr-mcp");
    Command::new(bin).arg("migrate-rules").args(args).output().unwrap()
}

#[test]
fn migrate_converts_v1_to_v2_in_place() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rules.json");
    std::fs::write(&path, r#"{ "rules": [
        { "id": "r1", "phase": "pre", "priority": 10,
          "conditions": [ {"predicate":"new_content_contains","args":[".unwrap()"]} ],
          "actions": [ {"action_type":"constraint_violation","params":["no unwrap"]} ] }
    ] }"#).unwrap();

    let out = run_migrate(&[path.to_str().unwrap()]);
    assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));

    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("\"when\""));
    assert!(text.contains("\"then\""));
    assert!(text.contains("\"block\""));
    assert!(!text.contains("\"action_type\""));
    // Backup preserved.
    assert!(path.with_extension("json.bak").exists());
}

#[test]
fn migrate_preserves_or_clauses() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rules.json");
    std::fs::write(&path, r#"{ "rules": [
        { "id": "r1", "phase": "pre", "priority": 5,
          "when": [ { "or": [ { "new_content_contains": "a" }, { "new_content_contains": "b" } ] } ],
          "then": { "warn": "m" } }
    ] }"#).unwrap();
    let out = run_migrate(&[path.to_str().unwrap()]);
    assert!(out.status.success());
    let text = std::fs::read_to_string(&path).unwrap();
    // OR is preserved on disk, NOT expanded.
    assert!(text.contains("\"or\""));
    assert!(!text.contains("#or0"));
}

#[test]
fn migrate_check_exit_codes() {
    let dir = tempfile::tempdir().unwrap();
    let v1 = dir.path().join("v1.json");
    std::fs::write(&v1, r#"{ "rules": [ { "id":"r","phase":"pre","priority":1,
        "conditions":[{"predicate":"p","args":["x"]}],
        "actions":[{"action_type":"log","params":["m"]}] } ] }"#).unwrap();
    let out = run_migrate(&["--check", v1.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1), "v1 file should report needs-migration");

    let v2 = dir.path().join("v2.json");
    std::fs::write(&v2, r#"{ "rules": [ { "id":"r","phase":"pre","priority":1,
        "when":[{"p":"x"}], "then":{"log":"m"} } ] }"#).unwrap();
    let out = run_migrate(&["--check", v2.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(0), "v2 file should report up-to-date");
}
```

- [ ] **Step 2: Run, verify failure**

Run: `cargo test -p phronesis-mcp --test migrate_integration 2>&1 | tail -15`
Expected: failures — `migrate-rules` subcommand doesn't exist (clap errors, non-zero exit, no `when` in output).

- [ ] **Step 3: Add the subcommand + handler**

In the `Command` enum in `main.rs` (after `MemoryDrift`):

```rust
    /// Convert a rules.json file from the v1 (predicate/args/action_type)
    /// shape to the v2 (when/then/predicate-as-key) shape. Preserves `or`
    /// clauses on disk (does not expand them). Idempotent.
    #[command(name = "migrate-rules")]
    MigrateRules {
        /// Path to the rules.json file to convert.
        path: PathBuf,
        /// Print the converted JSON to stdout; write nothing.
        #[arg(long)]
        dry_run: bool,
        /// Exit 0 if already v2, 1 if v1 (no writes). For CI gating.
        #[arg(long)]
        check: bool,
    },
```

In the match in `main()`:

```rust
        Command::MigrateRules { path, dry_run, check } => {
            use phronesis_mcp::rules_file::{self, SourceRule};

            // Read raw to detect shape: a rule with "conditions" is v1.
            let raw = std::fs::read_to_string(&path).map_err(|e| {
                anyhow::anyhow!("cannot read {}: {}", path.display(), e)
            })?;
            let parsed: serde_json::Value = serde_json::from_str(&raw)
                .map_err(|e| anyhow::anyhow!("malformed rules file: {}", e))?;
            let is_v1 = parsed
                .get("rules")
                .and_then(|r| r.as_array())
                .map(|arr| arr.iter().any(|r| r.get("conditions").is_some()))
                .unwrap_or(false);

            if check {
                if is_v1 {
                    eprintln!("{}: pre-v2 schema — run `phr-mcp migrate-rules` to convert", path.display());
                    std::process::exit(1);
                } else {
                    eprintln!("{}: already v2", path.display());
                    std::process::exit(0);
                }
            }

            // Parse to SourceRules (preserves OR), re-emit as v2.
            let sources: Vec<SourceRule> = rules_file::read_source(&path)
                .map_err(|e| anyhow::anyhow!("{}", e))?;

            if dry_run {
                #[derive(serde::Serialize)]
                struct Wrapper<'a> { rules: &'a [SourceRule] }
                println!("{}", serde_json::to_string_pretty(&Wrapper { rules: &sources })?);
                return Ok(());
            }

            rules_file::write_source(&path, &sources).map_err(|e| anyhow::anyhow!("{}", e))?;
            println!("migrated {} ({} rule(s)) to v2", path.display(), sources.len());
            Ok(())
        }
```

Ensure `SourceRule` and `write_source`/`read_source` are `pub` in `rules_file.rs` (they are, per Commit 1).

- [ ] **Step 4: Run, verify pass**

Run: `cargo test -p phronesis-mcp --test migrate_integration 2>&1 | tail -15`
Expected: 3 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/phronesis-mcp/src/main.rs crates/phronesis-mcp/tests/migrate_integration.rs
git commit -m "feat: phr-mcp migrate-rules (v1→v2, preserves or, --dry-run, --check)"
```

---

## COMMIT 4 — default pack + fixtures + version bump

### Task 4.1: Rewrite default rule packs in `init.rs` to v2 shape

**Files:**
- Modify: `crates/phronesis-mcp/src/init.rs` (all `*_rules()` functions)

- [ ] **Step 1: Confirm the existing init integration test covers shape**

The test `init_creates_all_five_files_in_fresh_project` reads `rules.json` and checks rule ids. After the rewrite, `init` writes v2 (because the JSON literals are v2). The test parses with `serde_json` directly into a `Value` and checks `rules[].id`, which is shape-agnostic — so it keeps passing. Good. We add a stricter assertion in step 3.

- [ ] **Step 2: Rewrite each rule literal from v1 to v2**

Mechanical transform applied to every rule in `llm_rules`, `rust_rules`, `rhai_rules`, `python_rules`, `typescript_rules`, `swift_rules`:

- `"conditions": [ {"predicate":"P","args":["A"]} ]` → `"when": [ {"P":"A"} ]`
- multi-arg `{"predicate":"P","args":["A","B"]}` → `{"P":["A","B"]}`
- `"actions": [{"action_type":"constraint_violation","params":["MSG"]}]` → `"then": {"block":"MSG"}`
- `constraint_warning` → `"then": {"warn":"MSG"}`

Example (the unwrap rule):

```rust
{
    "id": "enforce-no-unwrap-in-src",
    "phase": "pre", "priority": 10, "audit": true,
    "when": [
        {"new_content_contains": ".unwrap()"},
        {"file_path_matches": "src"}
    ],
    "then": {"block": "Avoid .unwrap() in src/ — use ? for error propagation, or expect() with a clear message if truly unreachable."}
}
```

Work through every rule in every pack the same way. The `__script__` predicate in rhai rules becomes `{"__script__": "..."}` if present (check the rhai pack — current rhai rules use `new_content_contains`/`file_extension_is`, no script literal, so straightforward).

- [ ] **Step 3: Strengthen the init test to assert v2 shape**

In `crates/phronesis-mcp/tests/init_integration.rs`, in `init_creates_all_five_files_in_fresh_project`, after the existing assertions:

```rust
    let raw = std::fs::read_to_string(dir.path().join(".phronesis/rules.json")).unwrap();
    assert!(raw.contains("\"when\""), "init must emit v2 `when`");
    assert!(raw.contains("\"then\""), "init must emit v2 `then`");
    assert!(!raw.contains("\"action_type\""), "init must not emit v1 `action_type`");
```

- [ ] **Step 4: Run, verify pass**

Run: `cargo test -p phronesis-mcp --test init_integration 2>&1 | tail -15`
Expected: all init tests pass; the new assertions confirm v2.

- [ ] **Step 5: Commit**

```bash
git add crates/phronesis-mcp/src/init.rs crates/phronesis-mcp/tests/init_integration.rs
git commit -m "feat(init): default rule packs emit v2 shape"
```

---

### Task 4.2: Convert test-fixture JSON literals to v2

**Files:**
- Modify: `crates/phronesis-mcp/tests/{action_log_integration,hook_integration,save_rules_integration,section_context_integration,bdd}.rs` and any `.feature` files with embedded rule JSON

- [ ] **Step 1: Inventory remaining v1 literals**

Run: `grep -rn '"action_type"\|"conditions"\|"predicate"' crates/phronesis-mcp/tests/ | grep -v migrate_integration`
Expected: a list of fixtures still in v1 shape. (Keep `migrate_integration.rs`'s v1 literals — they test the v1→v2 conversion and the v1 compat path on purpose.)

- [ ] **Step 2: Convert each remaining literal to v2**

Apply the same transform as Task 4.1, in place, per fixture. The common one:

`{"conditions":[{"predicate":"new_content_contains","args":["X"]}],"actions":[{"action_type":"log","params":["m"]}]}`
→ `{"when":[{"new_content_contains":"X"}],"then":{"log":"m"}}`

Keep **one** explicitly-v1 fixture somewhere (e.g. in `hook_integration.rs`) named to make the intent obvious — e.g. a test `v1_legacy_rules_still_load` — so backward compatibility is covered at the integration layer.

- [ ] **Step 3: Run the full workspace test suite**

Run: `cargo test --workspace --tests 2>&1 | grep -E '^test result|FAILED' | tail -30`
Expected: all green, zero failures.

- [ ] **Step 4: Run again to confirm count**

Run: `cargo test --workspace --tests 2>&1 | grep -E '^test result: ok' | awk '{p+=$4} END{print "passed:", p}'`
Expected: count ≥ prior 584 (new tests added, none removed).

- [ ] **Step 5: Commit**

```bash
git add crates/phronesis-mcp/tests/
git commit -m "test: convert fixtures to v2 shape; keep one v1 compat fixture"
```

---

### Task 4.3: Version bump + CLAUDE.md docs

**Files:**
- Modify: `crates/phronesis-mcp/Cargo.toml:3`
- Modify: `crates/phronesis-mcp/CLAUDE.md`

- [ ] **Step 1: Bump version**

`Cargo.toml`: `version = "0.7.0"` → `version = "0.8.0"`.

- [ ] **Step 2: Document v2 + migrate-rules in CLAUDE.md**

Add a "Rule file format (v2)" subsection near the existing rules docs, showing the `when`/`then`/predicate-as-key shape, the `{"or": [...]}` operator, and the `phr-mcp migrate-rules` command. Add `cargo run -- migrate-rules <path>` to the Build & Run command list. Note the v1 compatibility (both shapes parse; writes emit v2).

- [ ] **Step 3: Build to confirm version compiles**

Run: `cargo build -p phronesis-mcp 2>&1 | tail -3`
Expected: Finished.

- [ ] **Step 4: Full test run**

Run: `cargo test --workspace --tests 2>&1 | grep -E '^test result: ok' | awk '{p+=$4;f+=$6} END{print "passed:",p,"failed:",f}'`
Expected: failed: 0.

- [ ] **Step 5: Commit**

```bash
git add crates/phronesis-mcp/Cargo.toml crates/phronesis-mcp/CLAUDE.md
git commit -m "chore: bump phronesis-mcp 0.8.0; document v2 schema + migrate-rules"
```

---

## Rollout (after all commits land)

These are operational steps, run once by the user (not part of any commit). They create public/disk changes, so honor the commit-timing window before running anything that writes.

- [ ] Reinstall the binary: `cargo install --path crates/phronesis-mcp` → `phr-mcp --version` shows 0.8.0.
- [ ] Migrate this project: `phr-mcp migrate-rules ~/Git/phronesis/.phronesis/rules.json` (the 2 commit-timing custom rules + 34 defaults → v2).
- [ ] Migrate rulgamr: `phr-mcp migrate-rules ~/Git/rulgamr/.phronesis/rules.json`.
- [ ] Sanity check both: `phr-mcp migrate-rules --check <path>` exits 0 for each.
- [ ] Confirm rulgamr's `cargo build` is unaffected (no `phr` library change): `cd ~/Git/rulgamr && cargo build`.

---

## Self-Review notes

- **Spec coverage:** v2 shape (Tasks 1.1–1.3), both-shapes parse (1.2, 1.4), OR unfold incl. all edge cases (2.1), end-to-end OR (2.2), migrate-rules with dry-run/check/preserve-OR (3.1), default-pack rewrite (4.1), fixture conversion + v1 compat fixture (4.2), version + docs (4.3), rollout to both projects (Rollout). All spec sections map to a task.
- **Deviation from spec wording:** the spec's pipeline says `read() → Vec<DiskRule> → unfold_or()`. The plan refines this: `read()` returns *already-unfolded* flat `DiskRule`s (unfold happens inside `read`), and a separate `read_source()` returns OR-bearing `SourceRule`s for migration. This avoids changing `DiskRule`'s shape and so avoids rippling into `audit.rs`/`merge`. Same outcome, cleaner isolation.
- **Bonus fix:** Task 1.5 removes the duplicate inline parser in `hook::load_rules` and thereby fixes a latent bug where script conditions were dropped (`script: None`) at hook time.
- **Plan location:** saved here under `docs/specs/` (alongside `SPEC-rule-schema-v2.md`) rather than the skill default `docs/superpowers/plans/`, to keep spec + plan adjacent in this repo's established docs structure.
