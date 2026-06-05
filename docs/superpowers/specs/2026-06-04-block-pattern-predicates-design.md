# Block-pattern surfacing predicates — design

**Status**: approved (brainstorm)
**Date**: 2026-06-04
**Successor**: implementation plan at `docs/superpowers/plans/2026-06-04-block-pattern-predicates.md` (to be written)

## Problem

John Nunley's "block pattern" — wrapping intermediate `let` bindings inside
a `let result = { ... };` block so only the final value escapes — is a
useful Rust idiom that cleans up namespace pollution, scopes mutability,
and leads with author intent. It cannot be detected by phronesis's
existing substring/AST predicates: the anti-shape is a *function with
many top-level intermediate bindings*, which requires AST traversal
plus scope awareness, not token matching.

A separate framing matters: phronesis is **an MCP server in dialogue
with an LLM**, not a stand-alone deterministic linter. That means
heuristic predicates that surface *candidate sites* are useful — the
model adjudicates each hit when running `phr-mcp audit` or while in
conversation. The signal/noise bar drops considerably because the
model can read each match and judge.

## Goals

1. Surface functions that are candidates for block-pattern refactoring
   via two new AST predicates: `function_let_mut_count_high` and
   `function_let_binding_count_high`.
2. Functions that have already adopted the block pattern must go
   silent (the rule does not punish the pattern it surfaces).
3. Slot into the existing `syntax/` extractor pipeline with no
   structural change to the engine, hook, or audit runner.
4. Pair with two audit-phase rules in the Rust seed pack, two ADRs,
   and a paraphrased "Block Pattern" entry in `RUST-PATTERNS-GUIDE.md`
   that replaces the verbatim Nunley copy currently in the upstream
   working document.

## Non-goals

- Real variable-liveness analysis (the proposed
  `function_mut_binding_frozen_after` predicate). Deferred to a
  follow-up; not in scope for this design.
- Adjacent counter predicates (`function_line_count_high`,
  `struct_field_count_high`, `function_match_arm_count_high`,
  `function_nesting_depth_high`). Also deferred — the pattern this
  design establishes will inform them.
- Hook-time (`pre`-phase) warnings. Both new rules are audit-only.

## Architecture

Three files change. The pipeline stays:

```
edited file ─► syntax::extract() ─► SyntaxFacts ─► all_facts() ─► RETE assert ─► rules fire
```

No changes to `mod.rs`, `parsed.rs`, the RETE engine, `hook.rs`,
`audit.rs`, or `context.rs`.

### `crates/phronesis-mcp/src/syntax/rust.rs`

Add two new public-to-crate extractor functions plus one shared private
helper:

```rust
pub(crate) fn extract_function_let_mut_counts_high(parsed: &ParsedFile)
    -> Vec<(String, usize)>;

pub(crate) fn extract_function_let_binding_counts_high(parsed: &ParsedFile)
    -> Vec<(String, usize)>;

fn count_outer_scope_let_declarations<F>(
    node: tree_sitter::Node,
    source: &[u8],
    matches: &F,
    count: &mut usize,
) where F: Fn(tree_sitter::Node, &[u8]) -> bool;
```

The helper implements the scope-aware walk that makes block-pattern
adopters silent:

```rust
fn count_outer_scope_let_declarations<F>(
    node: tree_sitter::Node,
    source: &[u8],
    matches: &F,
    count: &mut usize,
)
where F: Fn(tree_sitter::Node, &[u8]) -> bool,
{
    // Halt at scope boundaries — bindings beyond these belong to
    // an independent scope.
    match node.kind() {
        "function_item"      => return, // nested fn — independent unit
        "block_expression"   => return, // Nunley's pattern lives here
        "closure_expression" => return, // closures own their scope
        _ => {}
    }

    if node.kind() == "let_declaration" && matches(node, source) {
        *count += 1;
        // Continue walking — `let x = { let y = ...; }` still counts `x`.
    }

    let mut walker = node.walk();
    for child in node.children(&mut walker) {
        count_outer_scope_let_declarations(child, source, matches, count);
    }
}
```

The two extractors wire it up:

```rust
const LET_MUT_THRESHOLD: usize = 3;
const LET_BINDING_THRESHOLD: usize = 8;

pub(crate) fn extract_function_let_mut_counts_high(parsed: &ParsedFile)
    -> Vec<(String, usize)>
{
    let ParsedFile::Rust { tree, source } = parsed else { return Vec::new(); };
    let mut out = Vec::new();
    let mut walker = tree.walk();
    walk_function_items(&mut walker, source.as_bytes(), &mut |fn_node, name| {
        let Some(body) = fn_node.child_by_field_name("body") else { return; };
        let mut count = 0usize;
        count_outer_scope_let_declarations(
            body,
            source.as_bytes(),
            &|node, _| has_mut_keyword(node),
            &mut count,
        );
        if count >= LET_MUT_THRESHOLD {
            out.push((name.to_string(), count));
        }
    });
    out
}
// (binding counterpart identical except &|_, _| true and LET_BINDING_THRESHOLD)
```

### `crates/phronesis-mcp/src/syntax/facts.rs`

Two new fields and two new emission blocks in `all_facts()`:

```rust
/// Functions with `LET_MUT_THRESHOLD` (3) or more outer-scope `let mut`
/// declarations. Block-expression and closure bodies are NOT recursed
/// into, so functions that already adopted the block pattern go silent.
pub function_let_mut_counts_high: Vec<(String, usize)>,

/// Functions with `LET_BINDING_THRESHOLD` (8) or more outer-scope `let`
/// declarations. Same scope-walk semantics as above.
pub function_let_binding_counts_high: Vec<(String, usize)>,
```

```rust
for (i, (fn_name, count)) in self.function_let_mut_counts_high.iter().enumerate() {
    out.push(Fact {
        id: format!("function_let_mut_count_high_{}_{}", fn_name, i),
        predicate: "function_let_mut_count_high".to_string(),
        args: vec![file_path.to_string(), fn_name.clone(), count.to_string()],
        timestamp: 0,
    });
}
// (binding counterpart identical, with predicate name "function_let_binding_count_high")
```

`rust::extract()` is wired to populate both fields by calling the new
extractors (same shape as the existing `function_clone_counts_high`
line).

### `crates/phronesis-mcp/src/init.rs`

Two new entries in the `rust_rules()` JSON value, both audit-phase:

```jsonc
{
  "id": "audit-rust-let-mut-count-high",
  "phase": "audit",
  "priority": 3,
  "audit": true,
  "when": [
    {"function_let_mut_count_high": ["?file", "?fn", "?count"]}
  ],
  "then": {"warn": "`?fn` in ?file has ?count outer-scope `let mut` declarations — consider John Nunley's block pattern: wrap the mutation in `let x = { let mut tmp = ...; ...; tmp }` so the surrounding scope sees an immutable binding."}
},
{
  "id": "audit-rust-let-binding-count-high",
  "phase": "audit",
  "priority": 3,
  "audit": true,
  "when": [
    {"function_let_binding_count_high": ["?file", "?fn", "?count"]}
  ],
  "then": {"warn": "`?fn` in ?file has ?count outer-scope `let` bindings — consider scoping intermediate temporaries into a block (`let result = { let raw = ...; let parsed = ...; ... }`) so only the final value is visible to the rest of the function. See RUST-PATTERNS-GUIDE.md §Block Pattern."}
}
```

## Scope-walker semantics

| Construct | Recurse? | Why |
|---|---|---|
| `function_item` (nested fn) | **No** | Independent unit, walked separately |
| `block_expression` (`{ ... }`) | **No** | Block-pattern adopters live here |
| `closure_expression` | **No** | Closures own their scope |
| `if_expression` / `match_arm` | **Yes** | Conditional outer-scope flow |
| `for_expression` / `while_expression` / `loop_expression` | **Yes** | Looping outer-scope flow |
| `unsafe` / `async` blocks | **No** (same as `block_expression`) | Use `block_expression` under the hood |
| `if let` / `while let` pattern bindings | **N/A** | Not `let_declaration` nodes |

## Thresholds

- `LET_MUT_THRESHOLD = 3` — matches `function_clone_counts_high`
  convention; two `let mut` is normal, three starts to smell.
- `LET_BINDING_THRESHOLD = 8` — captures the "long ladder" smell
  without firing on short functions that have a handful of natural
  intermediates.

Both are `const` in `rust.rs`, tweakable by recompile but not by
config. (Future work could promote to runtime tunables — out of scope
for this design.)

## Testing strategy

### Layer 1 — extractor unit tests in `syntax/rust.rs`

Inline source-string fixtures, one per shape:

| Fixture | Asserts |
|---|---|
| `LADDER_SOURCE` (8 outer-scope lets) | `binding_count_high` fires with count 8 |
| `BLOCK_ADOPTER_SOURCE` (1 outer + 7 inside `let x = { ... }`) | `binding_count_high` does NOT fire |
| `MUT_HEAVY_SOURCE` (3 outer-scope `let mut`) | `mut_count_high` fires with count 3 |
| `MUT_IN_BLOCK_SOURCE` (0 outer + 3 nested) | `mut_count_high` does NOT fire |
| `IF_LET_SOURCE` | `if let Some(x)` does not count; nested `let y` does |
| `MATCH_ARM_SOURCE` | Lets inside match arms count (match arms recurse) |
| `CLOSURE_BODY_SOURCE` | Lets inside `.map(|x| { ... })` do not count |
| `NESTED_FN_SOURCE` | Inner-fn lets count toward inner, not outer |

### Layer 2 — `facts.rs` flatten test

Two small tests asserting that `all_facts()` emits the right predicate
names and arg ordering for each new fact family. Pattern matches the
existing `function_clone_counts_high` flatten test.

### Layer 3 — pack integration test in `init.rs`

Extend (or add) a test asserting the two new rule IDs land in
`Pack::Rust.rules()`. Pattern matches the existing
`swift_pack_yields_rules` shape.

No new test harness. `cargo test --workspace` continues to be the
single command.

## Rule pack integration

Two new rules in `rust_rules()` as shown above. Pack remains a
single `Value` in `init.rs`.

## Documentation

Three doc surfaces touched:

1. **Two ADRs** in `.phronesis/wiki/decisions/`:
   - `2026-06-04-rust-let-mut-count-high.md` — enforces
     `audit-rust-let-mut-count-high`; cites Nunley's "erasure of
     mutability" benefit; notes the block-pattern-adopter-silence
     property.
   - `2026-06-04-rust-let-binding-count-high.md` — enforces
     `audit-rust-let-binding-count-high`; cites Nunley's
     "intent-first / namespace cleanliness" benefits.

2. **`crates/phronesis-mcp/docs/RUST-PATTERNS-GUIDE.md`** gains a
   "Block Pattern" entry (2 paragraphs, paraphrased from Nunley's
   post with link). Replaces the verbatim copy currently in the
   upstream working document; the working document gets the same
   paraphrase via a separate commit in the rulgamr repo.

3. **`NOTICES.md`** keeps the Nunley entry — we are now drawing on
   his idea (via paraphrase + ADR citation), so the debt is real.
   Update the entry text to match: instead of saying "phronesis
   ships no rule derived from it," reflect that two audit-phase
   rules are now derived from the idea.

4. **`crates/phronesis-mcp/README.md`** Rust-pack bullet gets two
   line items added: *"audit-only: ... `let mut`-heavy functions
   (block-pattern candidate), `let`-heavy functions (block-pattern
   candidate)."*

## Branch / release strategy

- Per `feedback_tag_pre_feature` memory: tag `main` at the current
  release version (read `Cargo.toml`) BEFORE starting feature work.
- Feature branch: `feature/block-pattern-predicates` (worktree per
  `superpowers:using-git-worktrees`).
- Swift work currently uncommitted on main: land first via separate
  commits on main, BEFORE creating the feature branch and tag.
  Sequence:
  1. Commit Swift pack additions (rule pack + tests + 5 ADRs + NOTICES)
  2. Tag main (pre-feature-state anchor)
  3. Create feature branch / worktree
  4. Implement block-pattern feature

## Risks

- **Tree-sitter-rust node-kind drift**: future grammar updates
  could rename `block_expression` or split it. Mitigation: the
  tests cover the silence-when-block-adopted property, so a
  grammar rename would surface as failing fixtures rather than
  silent wrong behavior.
- **False-positive noise**: `let_binding_count_high` at threshold 8
  may still fire on legitimate long functions (constructors of
  complex types, parsers). Audit-only phase limits the blast radius
  — surfaced only on demand, not on every edit. Adjust the
  threshold up if real-world audit runs are noisy.
- **`unsafe`/`async` blocks as scope-pattern hiding mechanism**:
  exotic edge case; not worth special-casing.

## Open questions (none blocking)

- Should `LET_BINDING_THRESHOLD` be 6, 8, or 10 by default? Going
  with **8**; tweak after first real-world audit runs.
- Eventually expose thresholds as rule arguments (`?threshold`)?
  Not in this design; out of scope.
