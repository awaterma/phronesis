# Block-Pattern Predicates Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship two new AST count predicates — `function_let_mut_count_high` (threshold 3) and `function_let_binding_count_high` (threshold 8) — wired into the Rust seed pack as audit-phase rules so phronesis can surface candidate sites for John Nunley's block-pattern refactor.

**Architecture:** Add two extractors to `crates/phronesis-mcp/src/syntax/rust.rs` that share a private scope-aware walker (`count_outer_scope_let_declarations`) that halts at `block_expression`, `closure_expression`, and `function_item` so functions that already adopted the block pattern go silent. Wire the two count fields into `SyntaxFacts` and emit them in `all_facts()`. Add two audit-phase rules to `rust_rules()`. Pair each with an ADR that directly cites Nunley's "Rust's Block Pattern" (Dec 2025) as the canonical source.

**Tech Stack:** Rust, tree-sitter-rust (existing dependency), `cargo test` for verification, `git` for tagging and worktree-based isolation.

**Reference spec:** `docs/superpowers/specs/2026-06-04-block-pattern-predicates-design.md` (commits `9ff52d5` + `7af5877`).

---

## Phase 0: Pre-feature housekeeping

Lands uncommitted Swift work on `main`, tags the pre-feature state, and creates an isolated worktree branch for the predicate work. Per the user's `feedback_tag_pre_feature` memory: tag main at the current release version before any feature-branch implementation begins.

### Task 0.1: Land Swift pack additions on main

**Files:**
- Modify: `crates/phronesis-mcp/src/init.rs` (already modified — Swift pack expansion, 2 → 7 rules)
- Create: `.phronesis/wiki/decisions/2026-06-04-swift-force-cast.md`
- Create: `.phronesis/wiki/decisions/2026-06-04-swift-fatal-error.md`
- Create: `.phronesis/wiki/decisions/2026-06-04-swift-mutable-singleton.md`
- Create: `.phronesis/wiki/decisions/2026-06-04-swift-legacy-constructor.md`
- Create: `.phronesis/wiki/decisions/2026-06-04-swift-legacy-random.md`
- Create: `NOTICES.md`

- [ ] **Step 1: Verify the Swift work is still green**

Run:
```bash
cargo test --quiet -p phronesis-mcp --lib -- init::
```
Expected: `41 passed; 0 failed`.

- [ ] **Step 2: Bump version (PATCH for rule-pack tweak)**

Per `crates/phronesis-mcp/CLAUDE.md`: "PATCH (0.X.Y) — bug fixes, internal refactors, doc-only changes, rule pack tweaks that don't add a new rule kind."

Edit `crates/phronesis-mcp/Cargo.toml` line 3:
```toml
version = "0.9.1"
```

Edit `crates/phronesis/Cargo.toml` line 3:
```toml
version = "0.9.1"
```

- [ ] **Step 3: Verify build still passes after version bump**

Run:
```bash
cargo build --workspace
```
Expected: clean build, no errors.

- [ ] **Step 4: Stage and commit Swift pack + ADRs + NOTICES**

Stage the specific paths (avoid `git add .` because the spec for *this* plan is already committed and there's nothing else loose):

```bash
git add crates/phronesis-mcp/src/init.rs \
        crates/phronesis-mcp/Cargo.toml \
        crates/phronesis/Cargo.toml \
        .phronesis/wiki/decisions/2026-06-04-swift-force-cast.md \
        .phronesis/wiki/decisions/2026-06-04-swift-fatal-error.md \
        .phronesis/wiki/decisions/2026-06-04-swift-mutable-singleton.md \
        .phronesis/wiki/decisions/2026-06-04-swift-legacy-constructor.md \
        .phronesis/wiki/decisions/2026-06-04-swift-legacy-random.md \
        NOTICES.md
```

Then commit:
```bash
git commit -m "$(cat <<'EOF'
feat(swift): expand seed pack from 2 → 7 rules; add NOTICES

Swift pack additions sourced from eleev/swift-design-patterns and
SwiftLint's default-enabled rule set:

- warn-swift-force-cast (force-bang trio)
- audit-swift-fatal-error (precondition / assertionFailure / throws)
- audit-swift-mutable-singleton (Singleton pattern)
- audit-swift-legacy-constructor (SwiftLint default)
- audit-swift-legacy-random (SwiftLint default)

Five new ADRs under .phronesis/wiki/decisions/ wire `enforces:`
frontmatter to each rule ID. NOTICES.md acknowledges all upstream
sources (rust-unofficial/patterns MPL-2.0, eleev/swift-design-patterns
MIT, realm/SwiftLint MIT, plus an intermediate-doc lineage note for
the Rust pack).

Version bump 0.9.0 → 0.9.1 (PATCH for rule-pack tweak per
crates/phronesis-mcp/CLAUDE.md versioning rules).

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 5: Verify the commit landed and tests still pass**

Run:
```bash
git log --oneline -1
cargo test --quiet -p phronesis-mcp --lib -- init::
```
Expected: HEAD shows the swift commit; `41 passed; 0 failed`.

### Task 0.2: Tag pre-feature state

**Files:** none (git only)

- [ ] **Step 1: Tag main at v0.9.1**

```bash
git tag -a v0.9.1 -m "Pre-block-pattern-feature state: Swift pack expanded, NOTICES added"
```

- [ ] **Step 2: Verify the tag**

```bash
git tag --list 'v*'
git show --no-patch v0.9.1
```
Expected: `v0.8.1` and `v0.9.1` both listed; v0.9.1 points at the swift commit.

### Task 0.3: Create worktree feature branch

**Files:** none (git worktree only)

Per `superpowers:using-git-worktrees`: a worktree isolates feature work from the main checkout so partial work can't leak.

- [ ] **Step 1: Create worktree at sibling directory**

```bash
git worktree add -b feature/block-pattern-predicates ../phronesis-block-pattern v0.9.1
```

- [ ] **Step 2: Verify worktree**

```bash
git worktree list
```
Expected: two entries — the main checkout and `../phronesis-block-pattern` on `feature/block-pattern-predicates`.

- [ ] **Step 3: Switch to worktree for subsequent tasks**

```bash
cd ../phronesis-block-pattern
pwd
git rev-parse HEAD
```
Expected: working directory is `../phronesis-block-pattern`; HEAD matches the v0.9.1 commit.

**All subsequent tasks run from this worktree.** When the plan finishes, the worktree gets merged back to main and removed.

---

## Phase 1: Walker helper + first extractor (let-binding count)

The walker is the load-bearing part — gets the scope-aware semantics right once, both extractors call it.

### Task 1.1: Add failing test for `function_let_binding_counts_high` — ladder case

**Files:**
- Modify: `crates/phronesis-mcp/src/syntax/rust.rs` (test module at the bottom)

- [ ] **Step 1: Write the failing test**

Append at the end of the `#[cfg(test)] mod tests` block in `crates/phronesis-mcp/src/syntax/rust.rs`:

```rust
    #[test]
    fn let_binding_count_high_fires_on_long_ladder() {
        let code = "fn parse_config(path: &str) -> Result<Config> {
            let raw = fs::read(path)?;
            let s = String::from_utf8(raw)?;
            let stripped = strip_comments(&s);
            let json = unescape(&stripped);
            let parsed = serde_json::from_str(&json)?;
            let validated = validate(parsed)?;
            let normalized = normalize(validated);
            let final_cfg = expand_env(normalized);
            Ok(final_cfg)
        }";
        let facts = extract(code);
        assert_eq!(
            facts.function_let_binding_counts_high,
            vec![("parse_config".to_string(), 8)]
        );
    }
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test --quiet -p phronesis-mcp --lib -- syntax::rust::tests::let_binding_count_high_fires_on_long_ladder
```
Expected: compile error (the field doesn't exist yet) — `no field 'function_let_binding_counts_high' on type 'SyntaxFacts'`.

### Task 1.2: Add the `SyntaxFacts` field for the binding count

**Files:**
- Modify: `crates/phronesis-mcp/src/syntax/facts.rs:62` (add field to the struct before the Swift section)

- [ ] **Step 1: Add the field**

In `crates/phronesis-mcp/src/syntax/facts.rs`, find the line ending the Rust section (just before `// ─── Swift ──────`) and add:

```rust
    /// Functions with 8 or more *outer-scope* `let` declarations.
    /// Bindings inside child `block_expression` and `closure_expression`
    /// nodes are NOT counted, so functions that already adopted the
    /// block pattern (`let x = { let raw = ...; let parsed = ...; ... }`)
    /// go silent. Conditional and loop bodies (if/match/for/while/loop)
    /// DO recurse because they're continuations of the outer flow.
    /// Args: (fn_name, count). Threshold fixed at 8.
    pub function_let_binding_counts_high: Vec<(String, usize)>,
```

- [ ] **Step 2: Verify the field compiles (test still fails on extractor, but field is now declared)**

```bash
cargo build -p phronesis-mcp 2>&1 | tail -5
```
Expected: a warning that `function_let_binding_counts_high` is never set (the extractor isn't wired yet); no errors.

### Task 1.3: Add the walker helper and binding extractor

**Files:**
- Modify: `crates/phronesis-mcp/src/syntax/rust.rs` (alongside the existing `extract_function_clone_counts_high`)

- [ ] **Step 1: Add helper + extractor near `count_clone_calls`**

Find the end of `count_clone_calls` (around line 270 in `crates/phronesis-mcp/src/syntax/rust.rs`) and add immediately after it:

```rust
/// Recursive walk that respects function-scope boundaries: halts at
/// `block_expression`, `closure_expression`, and `function_item`.
///
/// Why these halts: a `let` inside a child block expression is the very
/// shape we want to *suggest* (the block pattern), so counting it
/// toward the outer function would punish the pattern this rule
/// surfaces. Closures own their scope. Nested functions are walked
/// separately by `walk_function_items`.
///
/// Recursion still descends into `if_expression`, `match_expression`,
/// `for_expression`, `while_expression`, and `loop_expression`
/// (default-recurse arm) because their bodies are continuations of the
/// outer function's control flow, not isolated scopes.
fn count_outer_scope_let_declarations<F>(
    node: tree_sitter::Node,
    source: &[u8],
    matches: &F,
    count: &mut usize,
) where
    F: Fn(tree_sitter::Node, &[u8]) -> bool,
{
    match node.kind() {
        "function_item" | "block_expression" | "closure_expression" => return,
        _ => {}
    }

    if node.kind() == "let_declaration" && matches(node, source) {
        *count += 1;
        // Continue descending — `let x = { let y = ...; }` should still
        // count `x` at the outer scope. The block halt above keeps `y`
        // from contributing.
    }

    let mut walker = node.walk();
    for child in node.children(&mut walker) {
        count_outer_scope_let_declarations(child, source, matches, count);
    }
}

/// Functions with 8 or more outer-scope `let` declarations.
/// See `count_outer_scope_let_declarations` for scoping semantics.
pub(crate) fn extract_function_let_binding_counts_high(
    parsed: &ParsedFile,
) -> Vec<(String, usize)> {
    const LET_BINDING_THRESHOLD: usize = 8;
    let ParsedFile::Rust { tree, source } = parsed else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut walker = tree.walk();
    walk_function_items(&mut walker, source.as_bytes(), &mut |fn_node, name| {
        let Some(body) = fn_node.child_by_field_name("body") else {
            return;
        };
        let mut count = 0usize;
        count_outer_scope_let_declarations(
            body,
            source.as_bytes(),
            &|_, _| true,
            &mut count,
        );
        if count >= LET_BINDING_THRESHOLD {
            out.push((name.to_string(), count));
        }
    });
    out
}
```

- [ ] **Step 2: Wire the extractor into `extract()`**

In `crates/phronesis-mcp/src/syntax/rust.rs`, find the `SyntaxFacts { ... }` construction inside `extract()` (around line 61) and add a line for the new field. Update from:

```rust
        function_clone_counts: extract_function_clone_counts(&parsed),
        function_clone_counts_high: extract_function_clone_counts_high(&parsed),
        pub_fns_without_doc_comment: extract_pub_fns_without_doc_comment(&parsed),
```

to:

```rust
        function_clone_counts: extract_function_clone_counts(&parsed),
        function_clone_counts_high: extract_function_clone_counts_high(&parsed),
        function_let_binding_counts_high: extract_function_let_binding_counts_high(&parsed),
        pub_fns_without_doc_comment: extract_pub_fns_without_doc_comment(&parsed),
```

- [ ] **Step 3: Run the test from Task 1.1**

```bash
cargo test --quiet -p phronesis-mcp --lib -- syntax::rust::tests::let_binding_count_high_fires_on_long_ladder
```
Expected: PASS.

### Task 1.4: Add the block-adopter test (the silence case)

This test pins the property that makes the rule worth shipping: a function that *already* adopted the block pattern goes silent.

**Files:**
- Modify: `crates/phronesis-mcp/src/syntax/rust.rs` (test module)

- [ ] **Step 1: Write the failing-but-expected-pass test**

Append after the previous test:

```rust
    #[test]
    fn let_binding_count_high_silent_on_block_adopter() {
        // The same logical work as `let_binding_count_high_fires_on_long_ladder`,
        // but scoped into a block expression. The block pattern adopter
        // should NOT fire.
        let code = "fn parse_config(path: &str) -> Result<Config> {
            let final_cfg = {
                let raw = fs::read(path)?;
                let s = String::from_utf8(raw)?;
                let stripped = strip_comments(&s);
                let json = unescape(&stripped);
                let parsed = serde_json::from_str(&json)?;
                let validated = validate(parsed)?;
                let normalized = normalize(validated);
                expand_env(normalized)
            };
            Ok(final_cfg)
        }";
        let facts = extract(code);
        assert!(
            facts.function_let_binding_counts_high.is_empty(),
            "block-pattern adopter must not fire; got {:?}",
            facts.function_let_binding_counts_high
        );
    }
```

- [ ] **Step 2: Run it**

```bash
cargo test --quiet -p phronesis-mcp --lib -- syntax::rust::tests::let_binding_count_high_silent_on_block_adopter
```
Expected: PASS (the walker halt at `block_expression` is what makes this work; if it doesn't pass, the halt isn't wired right).

### Task 1.5: Add the closure + nested-fn silence tests

**Files:**
- Modify: `crates/phronesis-mcp/src/syntax/rust.rs` (test module)

- [ ] **Step 1: Add three small fixture tests**

Append:

```rust
    #[test]
    fn let_binding_count_high_does_not_count_closure_lets() {
        let code = "fn host() {
            let _a = 1; let _b = 2; let _c = 3;
            let _d = 4; let _e = 5; let _f = 6;
            let _g = 7;
            let _result = items.iter().map(|x| {
                let y = x + 1;
                let z = y * 2;
                z
            }).collect::<Vec<_>>();
        }";
        // host has 8 outer-scope lets (_a.._g + _result) → fires.
        // The closure's let y / let z must NOT contribute, otherwise the
        // count would be 10.
        let facts = extract(code);
        assert_eq!(
            facts.function_let_binding_counts_high,
            vec![("host".to_string(), 8)]
        );
    }

    #[test]
    fn let_binding_count_high_nested_fn_counts_independently() {
        // Outer has 2 outer-scope lets (well below threshold);
        // inner has 8 (at threshold). Only inner fires.
        let code = "fn outer() {
            let _x = 1; let _y = 2;
            fn inner() {
                let _a = 1; let _b = 2; let _c = 3;
                let _d = 4; let _e = 5; let _f = 6;
                let _g = 7; let _h = 8;
            }
        }";
        let facts = extract(code);
        assert_eq!(
            facts.function_let_binding_counts_high,
            vec![("inner".to_string(), 8)]
        );
    }

    #[test]
    fn let_binding_count_high_counts_inside_if_and_match_arms() {
        // 4 outer + 2 in if + 2 in match = 8 → fires with count 8.
        // If `if` or `match` halted recursion, the count would be 4.
        let code = "fn flow(x: i32) {
            let _a = 1; let _b = 2; let _c = 3; let _d = 4;
            if x > 0 {
                let _e = 5;
                let _f = 6;
            }
            match x {
                0 => { let _g = 7; }
                _ => { let _h = 8; }
            }
        }";
        let facts = extract(code);
        assert_eq!(
            facts.function_let_binding_counts_high,
            vec![("flow".to_string(), 8)]
        );
    }
```

- [ ] **Step 2: Run all three**

```bash
cargo test --quiet -p phronesis-mcp --lib -- syntax::rust::tests::let_binding_count_high
```
Expected: all 5 tests in the `let_binding_count_high*` family pass.

### Task 1.6: Commit Phase 1

- [ ] **Step 1: Stage and commit**

```bash
git add crates/phronesis-mcp/src/syntax/rust.rs \
        crates/phronesis-mcp/src/syntax/facts.rs
git commit -m "$(cat <<'EOF'
feat(syntax): add function_let_binding_count_high predicate

Surfaces functions with 8+ outer-scope `let` declarations as
candidates for John Nunley's block-pattern refactor. The walker
halts at `block_expression`, `closure_expression`, and `function_item`
so block-pattern adopters go silent — the rule does not punish what
it surfaces. Conditional and loop bodies (if/match/for/while/loop)
still recurse.

Tests cover the ladder-fires, block-adopter-silent, closure-silent,
nested-fn-independent, and if/match-recurses shapes.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

- [ ] **Step 2: Verify the commit**

```bash
git log --oneline -1
```
Expected: HEAD shows the Phase 1 commit on `feature/block-pattern-predicates`.

---

## Phase 2: Second extractor (let-mut count)

Reuses the walker from Phase 1; only adds a `matches` predicate that checks for `mutable_specifier`.

### Task 2.1: Add failing test for `function_let_mut_counts_high`

**Files:**
- Modify: `crates/phronesis-mcp/src/syntax/rust.rs` (test module)

- [ ] **Step 1: Add the test**

Append:

```rust
    #[test]
    fn let_mut_count_high_fires_at_three() {
        let code = "fn mutable_heavy() {
            let mut a = vec![];
            let mut b = String::new();
            let mut c = 0;
            a.push(1); b.push_str(\"x\"); c += 1;
        }";
        let facts = extract(code);
        assert_eq!(
            facts.function_let_mut_counts_high,
            vec![("mutable_heavy".to_string(), 3)]
        );
    }
```

- [ ] **Step 2: Run to verify it fails**

```bash
cargo test --quiet -p phronesis-mcp --lib -- syntax::rust::tests::let_mut_count_high_fires_at_three
```
Expected: compile error — `function_let_mut_counts_high` field doesn't exist yet.

### Task 2.2: Add the `SyntaxFacts` field

**Files:**
- Modify: `crates/phronesis-mcp/src/syntax/facts.rs` (alongside the binding field added in Task 1.2)

- [ ] **Step 1: Add the field immediately after `function_let_binding_counts_high`**

```rust
    /// Functions with 3 or more *outer-scope* `let mut` declarations.
    /// Same scope semantics as `function_let_binding_counts_high`
    /// (halt at child blocks/closures, recurse into if/match/for/while).
    /// Args: (fn_name, count). Threshold fixed at 3.
    pub function_let_mut_counts_high: Vec<(String, usize)>,
```

- [ ] **Step 2: Verify it compiles**

```bash
cargo build -p phronesis-mcp 2>&1 | tail -5
```
Expected: warning that the new field is never set; no errors.

### Task 2.3: Add the mut extractor + mut-keyword helper

**Files:**
- Modify: `crates/phronesis-mcp/src/syntax/rust.rs` (near the binding extractor from Task 1.3)

- [ ] **Step 1: Add `has_mut_keyword` and the extractor**

Append immediately after `extract_function_let_binding_counts_high`:

```rust
/// True when a `let_declaration` node has a `mutable_specifier` child
/// (i.e., the `mut` keyword is present). Tree-sitter-rust grammar
/// represents `mut` as a sibling-of-pattern child, not a field.
fn has_mut_keyword(node: tree_sitter::Node, _source: &[u8]) -> bool {
    let mut walker = node.walk();
    for child in node.children(&mut walker) {
        if child.kind() == "mutable_specifier" {
            return true;
        }
    }
    false
}

/// Functions with 3 or more outer-scope `let mut` declarations.
pub(crate) fn extract_function_let_mut_counts_high(
    parsed: &ParsedFile,
) -> Vec<(String, usize)> {
    const LET_MUT_THRESHOLD: usize = 3;
    let ParsedFile::Rust { tree, source } = parsed else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut walker = tree.walk();
    walk_function_items(&mut walker, source.as_bytes(), &mut |fn_node, name| {
        let Some(body) = fn_node.child_by_field_name("body") else {
            return;
        };
        let mut count = 0usize;
        count_outer_scope_let_declarations(
            body,
            source.as_bytes(),
            &has_mut_keyword,
            &mut count,
        );
        if count >= LET_MUT_THRESHOLD {
            out.push((name.to_string(), count));
        }
    });
    out
}
```

- [ ] **Step 2: Wire it into `extract()`**

In `crates/phronesis-mcp/src/syntax/rust.rs`, update the `SyntaxFacts { ... }` construction in `extract()` to add the mut field immediately after the binding field added in Task 1.3:

```rust
        function_let_binding_counts_high: extract_function_let_binding_counts_high(&parsed),
        function_let_mut_counts_high: extract_function_let_mut_counts_high(&parsed),
```

- [ ] **Step 3: Run the Task 2.1 test**

```bash
cargo test --quiet -p phronesis-mcp --lib -- syntax::rust::tests::let_mut_count_high_fires_at_three
```
Expected: PASS.

### Task 2.4: Add the let-mut silence test

**Files:**
- Modify: `crates/phronesis-mcp/src/syntax/rust.rs` (test module)

- [ ] **Step 1: Add the test**

Append:

```rust
    #[test]
    fn let_mut_count_high_silent_on_mut_inside_block() {
        // The `let mut`s are scoped inside a block expression — the
        // outer function should see them as already-scoped and the rule
        // must NOT fire.
        let code = "fn frozen_after() {
            let result = {
                let mut a = vec![];
                let mut b = String::new();
                let mut c = 0;
                a.push(1); b.push_str(\"x\"); c += 1;
                (a, b, c)
            };
            use_result(&result);
        }";
        let facts = extract(code);
        assert!(
            facts.function_let_mut_counts_high.is_empty(),
            "mut-in-block-adopter must not fire; got {:?}",
            facts.function_let_mut_counts_high
        );
    }

    #[test]
    fn let_mut_count_high_below_threshold_silent() {
        // Two `let mut`s — below the threshold of 3.
        let code = "fn two_muts() {
            let mut a = vec![];
            let mut b = String::new();
            a.push(1); b.push_str(\"x\");
        }";
        let facts = extract(code);
        assert!(facts.function_let_mut_counts_high.is_empty());
    }
```

- [ ] **Step 2: Run both**

```bash
cargo test --quiet -p phronesis-mcp --lib -- syntax::rust::tests::let_mut_count_high
```
Expected: all 3 `let_mut_count_high*` tests pass.

### Task 2.5: Commit Phase 2

- [ ] **Step 1: Stage and commit**

```bash
git add crates/phronesis-mcp/src/syntax/rust.rs \
        crates/phronesis-mcp/src/syntax/facts.rs
git commit -m "$(cat <<'EOF'
feat(syntax): add function_let_mut_count_high predicate

Surfaces functions with 3+ outer-scope `let mut` declarations as
candidates for Nunley's "erasure of mutability" block-pattern shape.
Reuses count_outer_scope_let_declarations with has_mut_keyword as the
matches filter.

Tests cover fires-at-threshold, silent-when-mut-is-scoped-in-block,
and silent-below-threshold.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 3: Wire facts into emission + integration tests

`all_facts()` is where the in-memory `SyntaxFacts` becomes the flat `Vec<Fact>` that the RETE engine asserts.

### Task 3.1: Add failing test for `all_facts()` emission shape

**Files:**
- Modify: `crates/phronesis-mcp/src/syntax/facts.rs` (test module — find existing tests at the bottom of the file or add a `#[cfg(test)] mod tests` block if absent)

- [ ] **Step 1: Inspect the existing `facts.rs` test conventions**

```bash
grep -n "#\[cfg(test)\]\|#\[test\]" crates/phronesis-mcp/src/syntax/facts.rs | head -10
```
Expected: there's already a tests module at the bottom (or `all_facts` is exercised via the hook integration tests).

- [ ] **Step 2: Add a small flatten test**

If a `tests` module exists in `facts.rs`, append. Otherwise, add this at the file end:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_facts_emits_let_binding_count_high() {
        let facts = SyntaxFacts {
            function_let_binding_counts_high: vec![("foo".to_string(), 10)],
            ..Default::default()
        };
        let out = facts.all_facts("/tmp/src.rs");
        let hit = out
            .iter()
            .find(|f| f.predicate == "function_let_binding_count_high");
        assert!(hit.is_some(), "no function_let_binding_count_high fact emitted");
        let hit = hit.unwrap();
        assert_eq!(hit.args, vec!["/tmp/src.rs".to_string(), "foo".to_string(), "10".to_string()]);
    }

    #[test]
    fn all_facts_emits_let_mut_count_high() {
        let facts = SyntaxFacts {
            function_let_mut_counts_high: vec![("bar".to_string(), 4)],
            ..Default::default()
        };
        let out = facts.all_facts("/tmp/src.rs");
        let hit = out
            .iter()
            .find(|f| f.predicate == "function_let_mut_count_high");
        assert!(hit.is_some(), "no function_let_mut_count_high fact emitted");
        let hit = hit.unwrap();
        assert_eq!(hit.args, vec!["/tmp/src.rs".to_string(), "bar".to_string(), "4".to_string()]);
    }
}
```

- [ ] **Step 3: Run the tests — they should fail (predicates not emitted yet)**

```bash
cargo test --quiet -p phronesis-mcp --lib -- syntax::facts::tests
```
Expected: both new tests fail with "no function_let_*_count_high fact emitted".

### Task 3.2: Add the emission blocks in `all_facts()`

**Files:**
- Modify: `crates/phronesis-mcp/src/syntax/facts.rs` (inside `impl SyntaxFacts { fn all_facts(...) }`)

- [ ] **Step 1: Add the two emission blocks immediately after the existing `function_clone_counts_high` block (around line 120)**

```rust
        for (i, (fn_name, count)) in self.function_let_binding_counts_high.iter().enumerate() {
            out.push(Fact {
                id: format!("function_let_binding_count_high_{}_{}", fn_name, i),
                predicate: "function_let_binding_count_high".to_string(),
                args: vec![file_path.to_string(), fn_name.clone(), count.to_string()],
                timestamp: 0,
            });
        }

        for (i, (fn_name, count)) in self.function_let_mut_counts_high.iter().enumerate() {
            out.push(Fact {
                id: format!("function_let_mut_count_high_{}_{}", fn_name, i),
                predicate: "function_let_mut_count_high".to_string(),
                args: vec![file_path.to_string(), fn_name.clone(), count.to_string()],
                timestamp: 0,
            });
        }
```

- [ ] **Step 2: Re-run the Task 3.1 tests**

```bash
cargo test --quiet -p phronesis-mcp --lib -- syntax::facts::tests
```
Expected: both new tests pass.

### Task 3.3: Commit Phase 3

- [ ] **Step 1: Stage and commit**

```bash
git add crates/phronesis-mcp/src/syntax/facts.rs
git commit -m "$(cat <<'EOF'
feat(syntax): emit let-mut and let-binding facts in all_facts()

Flatten the two new SyntaxFacts fields into the predicate/args shape
the RETE engine consumes:

- function_let_binding_count_high(file, fn_name, count)
- function_let_mut_count_high(file, fn_name, count)

Per-fact unit tests added in facts.rs::tests.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 4: Rust seed pack additions

Two new audit-phase rules in `rust_rules()` plus a pinning integration test.

### Task 4.1: Add failing pinning test for the two new pack rule IDs

**Files:**
- Modify: `crates/phronesis-mcp/src/init.rs` (test module)

- [ ] **Step 1: Locate existing rust-pack test fixtures**

```bash
grep -n "rust_pack_includes_\|fn rust_pack_" crates/phronesis-mcp/src/init.rs
```
Expected: shows existing tests like `rust_pack_carries_only_rust_rules`, `rust_pack_includes_new_predicate_rules`, etc.

- [ ] **Step 2: Add a new pinning test**

Append a new test to the existing `#[cfg(test)] mod tests` block in `crates/phronesis-mcp/src/init.rs`:

```rust
    #[test]
    fn rust_pack_includes_block_pattern_rules() {
        let v = Pack::Rust.rules();
        let ids: Vec<&str> = v["rules"]
            .as_array()
            .expect("rules array")
            .iter()
            .map(|r| r["id"].as_str().expect("rule id is a string"))
            .collect();
        assert!(
            ids.contains(&"audit-rust-let-binding-count-high"),
            "expected audit-rust-let-binding-count-high in rust pack, got {:?}",
            ids
        );
        assert!(
            ids.contains(&"audit-rust-let-mut-count-high"),
            "expected audit-rust-let-mut-count-high in rust pack, got {:?}",
            ids
        );
    }
```

- [ ] **Step 3: Run the test to verify it fails**

```bash
cargo test --quiet -p phronesis-mcp --lib -- init::tests::rust_pack_includes_block_pattern_rules
```
Expected: FAIL — `expected audit-rust-let-binding-count-high in rust pack`.

### Task 4.2: Add the two rules to `rust_rules()`

**Files:**
- Modify: `crates/phronesis-mcp/src/init.rs` (inside `fn rust_rules() -> Value`)

- [ ] **Step 1: Find the end of the rules array in `rust_rules()` and add the two new rules**

Find the closing `]` of the `"rules": [ ... ]` array inside `rust_rules()` (around the end of that function, before the closing `})`). Insert these two entries before the closing `]` (mind the comma on the previous entry):

```jsonc
            ,
            {
                "id": "audit-rust-let-binding-count-high",
                "phase": "audit",
                "priority": 3,
                "audit": true,
                "when": [
                    {"function_let_binding_count_high": ["?file", "?fn", "?count"]}
                ],
                "then": {"warn": "`?fn` in ?file has ?count outer-scope `let` bindings — consider scoping intermediate temporaries into a block (`let result = { let raw = ...; let parsed = ...; ... }`) so only the final value is visible to the rest of the function. Block pattern: John Nunley, 'Rust's Block Pattern' (Dec 2025)."}
            },
            {
                "id": "audit-rust-let-mut-count-high",
                "phase": "audit",
                "priority": 3,
                "audit": true,
                "when": [
                    {"function_let_mut_count_high": ["?file", "?fn", "?count"]}
                ],
                "then": {"warn": "`?fn` in ?file has ?count outer-scope `let mut` declarations — consider John Nunley's block pattern: wrap the mutation in `let x = { let mut tmp = ...; ...; tmp }` so the surrounding scope sees an immutable binding."}
            }
```

(If the prior entry already has a trailing comma, omit the leading comma above.)

- [ ] **Step 2: Run the pinning test**

```bash
cargo test --quiet -p phronesis-mcp --lib -- init::tests::rust_pack_includes_block_pattern_rules
```
Expected: PASS.

- [ ] **Step 3: Run the full init test suite to catch regressions**

```bash
cargo test --quiet -p phronesis-mcp --lib -- init::
```
Expected: all init tests pass (count was 41 pre-feature; should be 42 after adding the new pinning test).

### Task 4.3: End-to-end smoke — exercise the rule via `phr-mcp audit`

A semi-manual verification that the full pipeline (file read → syntax extract → fact emit → rule fires → audit output) works.

**Files:**
- Create: `/tmp/block-pattern-smoke/src/main.rs`
- Create: `/tmp/block-pattern-smoke/Cargo.toml`
- Create: `/tmp/block-pattern-smoke/.phronesis/rules.json`

- [ ] **Step 1: Build the binary**

```bash
cargo build --release -p phronesis-mcp
ls target/release/phr-mcp
```
Expected: `target/release/phr-mcp` exists.

- [ ] **Step 2: Set up the smoke target**

```bash
mkdir -p /tmp/block-pattern-smoke/src /tmp/block-pattern-smoke/.phronesis
cat > /tmp/block-pattern-smoke/Cargo.toml <<'EOF'
[package]
name = "smoke"
version = "0.1.0"
edition = "2021"
EOF
cat > /tmp/block-pattern-smoke/src/main.rs <<'EOF'
fn ladder() {
    let _a = 1;
    let _b = 2;
    let _c = 3;
    let _d = 4;
    let _e = 5;
    let _f = 6;
    let _g = 7;
    let _h = 8;
}

fn block_adopter() {
    let _result = {
        let _a = 1;
        let _b = 2;
        let _c = 3;
        let _d = 4;
        let _e = 5;
        let _f = 6;
        let _g = 7;
        let _h = 8;
        99
    };
}

fn main() { ladder(); block_adopter(); }
EOF
```

- [ ] **Step 3: Initialize phronesis rules in the smoke project**

```bash
cd /tmp/block-pattern-smoke
"$OLDPWD/target/release/phr-mcp" init --packs rust --force
```
Expected: `.phronesis/rules.json` written with the rust pack including the two new rules.

- [ ] **Step 4: Run audit**

```bash
"$OLDPWD/target/release/phr-mcp" audit --rule audit-rust-let-binding-count-high
```
Expected: one match on `ladder` at the right line range; ZERO matches on `block_adopter` (the silence property).

- [ ] **Step 5: Return to the feature worktree**

```bash
cd "$OLDPWD"
pwd
```
Expected: back in `../phronesis-block-pattern`.

### Task 4.4: Commit Phase 4

- [ ] **Step 1: Stage and commit**

```bash
git add crates/phronesis-mcp/src/init.rs
git commit -m "$(cat <<'EOF'
feat(rules): add audit-rust-let-binding/mut-count-high rules

Two new audit-phase rules in the Rust seed pack, both citing John
Nunley's "Rust's Block Pattern" (Dec 2025) as the canonical source:

- audit-rust-let-binding-count-high  (fires at 8 outer-scope lets)
- audit-rust-let-mut-count-high      (fires at 3 outer-scope `let mut`s)

End-to-end smoke verified via phr-mcp audit on a fixture project:
the long-ladder function fires, the block-pattern adopter is silent.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 5: ADRs, NOTICES update, README bullet

Documentation that wires the citation chain.

### Task 5.1: ADR for `audit-rust-let-binding-count-high`

**Files:**
- Create: `.phronesis/wiki/decisions/2026-06-04-rust-let-binding-count-high.md`

- [ ] **Step 1: Write the ADR**

```markdown
---
id: rust-let-binding-count-high
date: 2026-06-04
status: accepted
enforces:
  - audit-rust-let-binding-count-high
superseded_by: null
tags: [rust, refactor, block-pattern, audit]
---

# Audit functions with long outer-scope `let` ladders

## Context

John Nunley's "Rust's Block Pattern" (December 2025) makes the case
for scoping intermediate `let` bindings inside a block expression:

```rust
let config = {
    let raw = fs::read(cfg_file)?;
    let s = String::from_utf8(raw)?;
    let stripped = strip_comments(&s);
    serde_json::from_str(&stripped)?
};
```

Three benefits Nunley names: the block leads with intent (`let config
= ...`), intermediate variables don't pollute the outer namespace, and
the intermediates drop at block end so resources release earlier.

We can't detect the *opportunity* via substring matching — the
anti-shape is a function with too many top-level intermediate `let`
bindings, which requires AST traversal plus scope awareness. The
predicate `function_let_binding_count_high` does that: counts
*outer-scope* `let_declaration` nodes, halting at child `block_expression`
and `closure_expression` nodes so functions that already adopted the
pattern go silent.

## Decision

Phase: `audit`. Threshold: 8 outer-scope `let` declarations. Fires
only under `phr-mcp audit`, surfacing candidate sites for the LLM
(or human reviewer) to judge.

The walker is deliberately conservative about scope boundaries:
`if`/`match`/`for`/`while`/`loop` bodies recurse (they're
continuations of the outer flow), but `{ ... }` block expressions
and closures halt. This is what makes the rule worth shipping —
it does not punish the very pattern it surfaces.

## Enforcement

`audit-rust-let-binding-count-high` runs only under `phr-mcp audit`.
The AST predicate `function_let_binding_count_high(file, fn, count)`
is the trigger.

## Consequences

- Long ladders surface as audit-table entries the model can read and
  judge per-function. False positives (constructors of complex types,
  parsers with naturally long intermediate stages) just get dismissed
  in conversation.
- Block-pattern adopters stay silent — the rule does not generate
  pressure to un-do the very refactor it suggests.
- If real-world audits prove noisy at threshold 8, the threshold
  lives in `crates/phronesis-mcp/src/syntax/rust.rs` as a `const`
  and is a one-line bump.
```

- [ ] **Step 2: Verify the file**

```bash
ls -la .phronesis/wiki/decisions/2026-06-04-rust-let-binding-count-high.md
head -10 .phronesis/wiki/decisions/2026-06-04-rust-let-binding-count-high.md
```
Expected: file exists with the frontmatter visible.

### Task 5.2: ADR for `audit-rust-let-mut-count-high`

**Files:**
- Create: `.phronesis/wiki/decisions/2026-06-04-rust-let-mut-count-high.md`

- [ ] **Step 1: Write the ADR**

```markdown
---
id: rust-let-mut-count-high
date: 2026-06-04
status: accepted
enforces:
  - audit-rust-let-mut-count-high
superseded_by: null
tags: [rust, refactor, block-pattern, mutability, audit]
---

# Audit functions with multiple outer-scope `let mut` declarations

## Context

A second benefit of John Nunley's "Rust's Block Pattern"
(December 2025) is *erasure of mutability*: a `let mut` inside a
block expression returns an immutable binding to the outer scope.

```rust
let data = {
    let mut data = vec![];
    data.push(1);
    data.extend_from_slice(&[4, 5, 6, 7]);
    data
};
// `data` is now immutable for the rest of the function.
```

Functions with three or more outer-scope `let mut` declarations are
candidates for this refactor — the mutability is often local to a
short build phase that could be scoped away.

This rule mirrors the design of
[rust-let-binding-count-high](2026-06-04-rust-let-binding-count-high.md),
sharing the same `count_outer_scope_let_declarations` walker but with
a `has_mut_keyword` filter so only `let mut` declarations count.

## Decision

Phase: `audit`. Threshold: 3 outer-scope `let mut` declarations.
Matches the precedent set by `function_clone_counts_high` (also 3).
Two `let mut`s are common; three start to suggest the block pattern
applies.

## Enforcement

`audit-rust-let-mut-count-high` runs only under `phr-mcp audit`.
The AST predicate `function_let_mut_count_high(file, fn, count)`
is the trigger.

## Consequences

- Functions with build-then-freeze patterns surface for review.
- Block-pattern adopters who already scoped their mutability stay
  silent.
- The rule is intentionally conservative: a `let mut` deep inside an
  `if` arm still counts (the mutability is still visible to the
  outer flow), but a `let mut` inside a `{ ... }` block expression
  is exempt.
```

- [ ] **Step 2: Verify the file**

```bash
ls -la .phronesis/wiki/decisions/2026-06-04-rust-let-mut-count-high.md
```

### Task 5.3: Update NOTICES.md

**Files:**
- Modify: `NOTICES.md` (the Nunley entry — currently says "phronesis ships no rule derived from it")

- [ ] **Step 1: Replace the Nunley paragraph**

Find this paragraph in `NOTICES.md` (under the **John Nunley** subsection):

```
The longer working document mentioned above also incorporates John
Nunley's blog post introducing the "block pattern" idiom. The
phronesis distribution of `RUST-PATTERNS-GUIDE.md` is truncated
before that section and ships no phronesis rule derived from it, but
the working document credits Nunley as the origin of that idiom and
we record the debt here for completeness.
```

Replace with:

```
The Rust pack's two audit-phase rules `audit-rust-let-binding-count-
high` and `audit-rust-let-mut-count-high` are derived directly from
John Nunley's "Rust's Block Pattern" post. The ADRs at
`.phronesis/wiki/decisions/2026-06-04-rust-let-{mut,binding}-count-
high.md` cite the post as their canonical source. Rule warning
messages link to the post inline.
```

- [ ] **Step 2: Verify the diff**

```bash
git diff NOTICES.md
```
Expected: the Nunley paragraph swap visible; no other changes.

### Task 5.4: Update README Rust-pack bullet

**Files:**
- Modify: `crates/phronesis-mcp/README.md` (the bullet describing the `rust` pack contents)

- [ ] **Step 1: Find the existing audit-only enumeration in the rust-pack bullet**

```bash
grep -n "Audit-only (silent at hook time" crates/phronesis-mcp/README.md
```
Expected: the line that lists the existing audit-only rules.

- [ ] **Step 2: Append the two new audit-only items**

In `crates/phronesis-mcp/README.md`, find the trailing portion of the audit-only enumeration that ends with `env::set_var(` in src/ (unsound under concurrent reads — and unsafe in edition 2024).` and immediately before the rust-unofficial/patterns attribution sentence (`The rust-unofficial/patterns book is the upstream source for ...`), add:

```
, functions with 3+ outer-scope `let mut` declarations (block-
pattern candidate: erasure of mutability via `let x = { let mut tmp =
...; tmp }`), functions with 8+ outer-scope `let` bindings (block-
pattern candidate: scope intermediate temporaries into a block)
```

- [ ] **Step 3: Verify the diff**

```bash
git diff crates/phronesis-mcp/README.md
```
Expected: a single edit inside the rust-pack bullet.

### Task 5.5: Run the full workspace test suite

A final regression check before we land Phase 5.

- [ ] **Step 1: Run all workspace tests**

```bash
cargo test --workspace --quiet 2>&1 | tail -20
```
Expected: all tests pass. The init pack tests now include the new pinning test; the syntax tests include the new fixtures; everything else unchanged.

- [ ] **Step 2: Run clippy on the workspace**

```bash
cargo clippy --workspace -- -D warnings 2>&1 | tail -20
```
Expected: zero warnings.

### Task 5.6: Commit Phase 5

- [ ] **Step 1: Stage and commit**

```bash
git add .phronesis/wiki/decisions/2026-06-04-rust-let-binding-count-high.md \
        .phronesis/wiki/decisions/2026-06-04-rust-let-mut-count-high.md \
        NOTICES.md \
        crates/phronesis-mcp/README.md
git commit -m "$(cat <<'EOF'
docs: ADRs + NOTICES + README for block-pattern predicates

Two new ADRs under .phronesis/wiki/decisions/ wire enforces:
frontmatter for the two new rule IDs, citing John Nunley's "Rust's
Block Pattern" (Dec 2025) directly. NOTICES.md updated to reflect
that two rules are now derived from the post (previously said
"phronesis ships no rule derived from it"). Rust-pack bullet in
README.md gains the two new audit-only line items.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Phase 6: Land the feature back to main

### Task 6.1: Merge feature branch to main

**Files:** none (git only)

- [ ] **Step 1: Switch back to the main checkout**

```bash
cd /Users/andrewwaterman/Git/phronesis
git status
```
Expected: clean working tree on `main`; HEAD at the Swift commit + spec commits.

- [ ] **Step 2: Merge the feature branch**

```bash
git merge --no-ff feature/block-pattern-predicates -m "$(cat <<'EOF'
Merge feature/block-pattern-predicates

Adds two AST count predicates (function_let_binding_count_high,
function_let_mut_count_high) plus paired Rust-pack rules that
surface block-pattern refactor candidates per John Nunley's
"Rust's Block Pattern" (Dec 2025).

See docs/superpowers/specs/2026-06-04-block-pattern-predicates-design.md
EOF
)"
```

- [ ] **Step 3: Verify the merge commit graph**

```bash
git log --graph --oneline -10
```
Expected: a merge commit with the feature-branch lineage visible.

### Task 6.2: Tear down the worktree

- [ ] **Step 1: Remove the worktree directory**

```bash
git worktree remove ../phronesis-block-pattern
git worktree list
```
Expected: only the main checkout remains.

- [ ] **Step 2: Final test pass on main**

```bash
cargo test --workspace --quiet 2>&1 | tail -5
cargo clippy --workspace -- -D warnings 2>&1 | tail -5
```
Expected: all tests pass, clippy clean.

### Task 6.3: Mark task #8 (broader patterns-roadmap spec) as ready to start

The block-pattern feature is now done. Task #8 in the session task list — the broader spec for patterns-guide → predicates coverage — becomes the natural next thread to pick up.

- [ ] **Step 1: Acknowledge follow-up**

(Conversational hand-off; no code change.) Surface to the user: "Block-pattern work landed and merged. Task #8 (broader patterns-roadmap spec) is now unblocked — invoke `superpowers:brainstorming` again when you want to start it."

---

## Self-review notes

- **Spec coverage:** Each section of the spec is implemented: the walker + scoping semantics in Phase 1 + Phase 2; data flow / emission in Phase 3; rule pack integration in Phase 4; documentation (ADRs + NOTICES update + README) in Phase 5; branch / release strategy in Phase 0 and Phase 6. The `RUST-PATTERNS-GUIDE.md`-is-NOT-updated decision from the spec edit at commit `7af5877` is honored — there's no task touching that file.
- **Placeholder scan:** no TBDs, no "implement later," every step has the actual code or command.
- **Type consistency:** field names plural (`function_let_binding_counts_high`), predicate names singular (`function_let_binding_count_high`) — matches existing `function_clone_counts_high` / `function_clone_count_high` precedent. Helper function name `count_outer_scope_let_declarations`, filter function `has_mut_keyword`. All references throughout the plan match.
- **Threshold consistency:** `LET_MUT_THRESHOLD = 3`, `LET_BINDING_THRESHOLD = 8`. Both `const`s live in `crates/phronesis-mcp/src/syntax/rust.rs` per the spec.
