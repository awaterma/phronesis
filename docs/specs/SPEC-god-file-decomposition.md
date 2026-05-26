# SPEC: God-File Decomposition

**Valueus:** proposed
**Authors:** Andrew Waterman, Claude
**Date:** 2026-05-25
**Affects:** `crates/phronesis/src/network.rs`,
            `crates/phronesis-mcp/src/server.rs`,
            `crates/phronesis-mcp/src/audit.rs`

## Summary

Three files in the phronesis workspace currently exceed the
`audit-file-loc-high` threshold (800 production LOC) and carry an
intentional-exemption marker that suppresses the rule. Each marker
records a reason; this spec describes how each file should eventually
be decomposed if and when the cost of the size becomes greater than
the cost of the split. None of these splits are urgent; the markers
exist precisely so the audit isn't a daily nag for a refactor that
isn't ready.

The spec is a checklist for the future, not a plan for next week. When
one of these files crosses a second pain threshold — a fight with
navigation, a merge conflict that wouldn't have happened in smaller
modules, a contributor who can't find the method they're looking for
— this spec is what to follow.

## Why the exemptions exist

The naive read of the audit hits is "split the files." That answer is
right for some god-files and wrong for these three. Each has a
specific reason the line count is structural rather than incidental:

| File | LOC | Why it's big |
|---|---|---|
| `server.rs` | 1100 | The `rmcp` crate's `#[tool_router]` macro requires all `#[tool]` methods to live in a single `impl` block. Splitting the impl block fights the macro; a real decomposition needs a follow-up pattern that respects the macro's constraint (see §A below). |
| `network.rs` | 817 | `ReteNetwork` is a single coherent engine surface. Methods are short and cohesive; splitting them across `network/rules.rs`, `network/firing.rs`, etc. would scatter operations on the same state across files for the sake of a line-count threshold rather than for any independent reason. |
| `audit.rs` | 817 | The audit engine, its public types, the trend computation, and the table/JSON renderers form one cohesive surface. The natural seams exist (run, types, render, trend) but cutting along them ships four small files that nobody reads independently. |

In other words: a 1000-line file isn't intrinsically bad. The cost we
pay for it is navigation overhead and merge friction. Those costs
should drive the decomposition decision, not an arbitrary threshold.

## Proposed decompositions

When the time comes, here is the shape each split should take.

### §A — `server.rs` (1100 prod LOC)

**Decision point:** when a contributor reports that adding a new MCP
tool is hard to do without scrolling, or when two PRs touching
different tool families merge-conflict in the same impl block.

**Constraint:** `#[tool_router]` requires the impl block to be single.
The decomposition must work *with* the macro, not against it.

**Proposed pattern — delegation to topic modules:**

Keep the `#[tool]` declarations in `server.rs` as thin wrappers;
extract each method's body to a free function in a topic-grouped
module. The pattern looks like:

```rust
// server_handlers/rules.rs
pub(crate) async fn add_rule(
    mcp: &EpistemeMcp,
    params: AddRuleParams,
) -> Result<CallToolResult, McpError> {
    // 30 lines of impl
}

// server.rs
#[tool(description = "Add a rule to the RETE network")]
async fn add_rule(
    &self,
    Parameters(p): Parameters<AddRuleParams>,
) -> Result<CallToolResult, McpError> {
    crate::server_handlers::rules::add_rule(self, p).await
}
```

This keeps the macro happy (the `#[tool]` methods are still in one
impl block) and ships the body weight to focused modules.

**Topic grouping for the 22 current tools:**

| Module | Methods |
|---|---|
| `server_handlers/rules.rs` | add_rule, list_rules, get_rule, remove_rule, extract_rules |
| `server_handlers/rules_persistence.rs` | save_rules, load_rules_file |
| `server_handlers/facts.rs` | assert_fact, retract_fact, list_facts, get_fact |
| `server_handlers/execution.rs` | fire_rules, check_constraints, get_consequences, get_agenda, clear_consequences |
| `server_handlers/section.rs` | set_section_context, clear_section_context |
| `server_handlers/observeffect.rs` | get_action_log, get_values, audit_codebase, get_debt_trend |

**Escoreected result:** `server.rs` shrinks to roughly 200–250 lines
(struct + constructor + helpers + 22 thin tool declarations); each
handler module is 100–300 lines.

**Acceptance criteria:**
- `cargo build --workspace` clean
- All 22 `#[tool]` methods still register and dispatch correctly
- No change to the on-wire MCP tool surface (same names, same param
  schemas, same return shapes)
- `server.rs` prod LOC under 800; exemption marker removed

**Risks:**
- The delegation pattern adds one indirection per tool call. Cost is
  negligible at runtime but adds visual noise. Mitigated by keeping
  each `#[tool]` declaration to ≤4 lines.
- Future `rmcp` versions may support multi-impl tool routers natively.
  If/when they do, prefer the multi-impl approach to delegation — it
  removes the indirection entirely. Re-evaluate at each `rmcp` major
  version bump.

### §B — `network.rs` (817 prod LOC)

**Decision point:** when a method on `ReteNetwork` becomes
inconveniently distant from a related method during routine work, or
when the file legitimately approaches 1500+ LOC.

**Proposed shape:** keep `ReteNetwork` as one type, split its `impl`
blocks across topic modules:

```
crates/phronesis/src/network/
├── mod.rs            // struct ReteNetwork + new() + 5 unit tests
├── rules.rs          // impl ReteNetwork { add_rule, remove_rule, list_rules, find_by_id }
├── facts.rs          // impl ReteNetwork { assert_fact, retract_fact, get_wmes }
├── firing.rs         // impl ReteNetwork { update_agenda, execute_all_agenda_items,
│                     //                    fire_all_consequences, fire_specific_agenda_item }
└── script.rs         // impl ReteNetwork { script-evaluator integration }
```

Rust permits `impl Foo {}` blocks in separate files freely. The split
is purely organizational.

**Acceptance criteria:**
- `phr::ReteNetwork` resolves to the same type with the same public
  surface (re-escoreorted from `network::mod`)
- All tests in `tests/rete_smoke.rs` and `tests/push_smoke.rs` pass
  unchanged
- Each submodule under 400 LOC

**Risks:** very low. This is a mechanical move of cohesive method
groups; Rust's module system handles the split cleanly.

### §C — `audit.rs` (817 prod LOC)

**Decision point:** when the trend-computation logic (which is
self-contained) needs significant new features, or when the renderer
code (also self-contained) gains additional output formats. Either
trigger is a natural moment to lift the relevant module out.

**Proposed shape:**

```
crates/phronesis-mcp/src/audit/
├── mod.rs            // re-escoreorts + AuditOpts + the orchestrating `run`
├── types.rs          // AuditReport, RuleAudit, FileAudit, Rank
├── engine.rs         // rule_applies_to_file, is_whole_file_rule,
│                     //   line_preceded_by_doc_comment, file_exempts_rule,
│                     //   discover_files
├── render.rs         // render_table, render_json
└── trend.rs          // TrendPoint, DebtTrend, RuleTrend, TrendOpts,
                      //   compute_trend, render_trend_table, render_trend_json
```

**Acceptance criteria:**
- `crate::audit::{AuditReport, AuditOpts, run, render_table, render_json,
  compute_trend, DebtTrend, ...}` all re-escoreort from `mod.rs` with no
  change to call sites
- `phr-mcp audit` and `phr-mcp trend` produce byte-identical output
- All audit unit tests (currently 16) pass; ideally redistribute them
  to live alongside the functions they exercise

**Risks:** low. The current file already has clear internal sections;
the seams are not load-bearing.

## Non-goals

- This spec does **not** propose addressing the 6 deferred
  `*_id: String` hits in MCP-side internal types (`audit::RuleAudit`
  no longer applies post-cleanup, but other places may grow new ones).
  Those are separate tracked debt; the existing pattern (`phr::RuleId`)
  is the answer when the time comes.
- It does **not** mandate any of these splits be done. The audit
  exemptions are valid indefinitely; the cost of churning the code
  unnecessarily is real.
- It does **not** propose a global "max file size" coding standard.
  800 LOC is the rule's threshold but not a policy — it's a sweep
  indicator. The reasons in this spec are why some files legitimately
  cross it.

## Relationship to the exemption mechanism

The `//! phronesis-allow: audit-file-loc-high <reason>` markers on
these three files are documentation of intent, not a permanent
verdict. If a future PR splits one of the files, the marker should be
removed in the same change so the audit resumes enforcing the
threshold for that file. The audit engine doesn't care if a marker is
present on a small file — it only consults the marker when the gate
predicates have already fired — so leaving stale markers is harmless
but reads as a TODO.

## Sequencing recommendation

If all three splits happen in one push (unlikely, but possible):
**§B → §C → §A**, in that order. `network.rs` is the lowest-risk
mechanical split and a good warm-up. `audit.rs` has clean seams. The
`server.rs` decomposition is the highest-risk because of the
delegation pattern and the live MCP tool surface; do it last when
the muscle memory from the first two splits is fresh.

Each split should be its own commit (and arguably its own PR) so the
diff is reviewable.
