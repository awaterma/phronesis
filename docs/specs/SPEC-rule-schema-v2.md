# SPEC: Rule schema v2 — readable wire format + OR operator

**Status:** draft
**Authors:** Claude, Andrew Waterman
**Date:** 2026-05-27
**Target release:** phronesis-mcp 0.8.0
**Affects:** `crates/phronesis-mcp/src/{rules_file.rs, init.rs, main.rs}`,
              test fixtures, `CLAUDE.md`. **Not** `crates/phronesis`
              (the `phr` library) — no engine or library-API change.

## Premise

The `.phronesis/rules.json` wire format is hard to read. A condition is
a positional `{"predicate": X, "args": [Y]}` object; an action is
`{"action_type": "constraint_violation", "params": [msg]}`. Reviewers
have to decode each condition against the predicate's argument
contract, and the action verb is buried behind a generic
`action_type` string. The format is the engine's internal
representation leaking onto disk.

This spec defines **schema v2**: a readable wire format keyed by
predicate and action verb, plus a first-class `OR` operator
implemented entirely in the rule loader (no engine change). It is
deliberately scoped to exclude `NOT`, which requires beta-network
work and a `phr`-library API change — that is a separate future spec
(see "Out of scope").

## Goals

- A wire format a reviewer can read without consulting a predicate
  argument table.
- First-class `{"or": [...]}` so disjunction is expressible without
  hand-duplicating rules.
- Zero disruption to existing projects (this repo, `~/Git/<consumer>`,
  any other `phr-mcp init`'d project) that have v1-shape rules.json on
  disk and share the globally-installed binary.
- No change to the `phr` library crate, so library consumers
  (the consumer depends on `phr` as a path dependency) are unaffected by a
  `cargo build`.

## Out of scope (deferred to later specs)

- **`NOT`** — requires a not-node in `phr::beta_network` plus a
  `Condition` API change that breaks struct-literal construction in
  library consumers. Its own spec, its own release (tentatively
  v0.9.0).
- **`AND` as an explicit wrapper** — AND is already implicit across
  the `when` array. An explicit `{"and": [...]}` only matters for
  composition *inside* `or`/`not`; revisit when NOT lands.
- **Named-argument predicates** — predicates stay positional. Named
  args (`{"function_param_count_high": {"file": ..., "fn": ...}}`)
  would require an argument-binding change in the engine; separate
  concern.
- **XOR / implication / universal quantification** — all derive from
  AND + OR + NOT, so they unlock once NOT exists. Nothing to build
  beyond the primitives.

## The v2 wire format

```json
{
  "rules": [
    {
      "id": "enforce-no-unwrap-in-src",
      "phase": "pre",
      "priority": 10,
      "audit": true,
      "when": [
        { "new_content_contains": ".unwrap()" },
        { "file_path_matches": "src" }
      ],
      "then": { "block": "Avoid .unwrap() in src/ — use ? for error propagation, or expect() with a clear message if truly unreachable." }
    },
    {
      "id": "warn-rust-test-via-bash",
      "phase": "pre",
      "priority": 5,
      "when": [
        { "or": [
          { "new_content_contains": "cargo test" },
          { "new_content_contains": "cargo nextest" }
        ]},
        { "file_extension_is": "rs" }
      ],
      "then": { "warn": "Running rust tests directly via Bash bypasses workspace cargo." }
    }
  ]
}
```

### Condition value shape (dispatch by arg count)

| Args | JSON value | Example |
|------|-----------|---------|
| 0    | `true`    | `{"some_zero_arg_predicate": true}` |
| 1    | string    | `{"new_content_contains": ".unwrap()"}` |
| 2+   | array     | `{"function_param_count_high": ["?file", "?fn", "?count"]}` |

The serializer chooses the shape from `args.len()`; the deserializer
accepts all three and normalizes back to `Vec<String>`. The
zero-arg boolean form is a forward-compatible placeholder — no
zero-arg predicate ships today, but the shape is defined so one can
be added without another format change. The value is ignored; only
the key (predicate name) matters.

### Script conditions

The existing `script` field (used by rhai-pack rules via the
`__script__` predicate) is expressed as:

```json
{ "__script__": "expr text" }
```

The deserializer routes a `__script__` key into
`DiskCondition.script` rather than `args`.

### Action verbs (`then`)

| v2 shape | v1 shape (internal action_type) |
|----------|--------------------------------|
| `{"block": "msg"}` | `constraint_violation` |
| `{"warn": "msg"}`  | `constraint_warning` |
| `{"log": "msg"}`   | `log` |

`then` is a single object, not an array. The internal
`Rule.actions: Vec<Action>` wraps the single action in a one-element
Vec. No rule in the codebase has ever had multiple actions; if needed
later, `then` can grow an array form behind an untagged enum without
breaking the single-object form.

### Rule-level metadata (unchanged)

`id`, `phase`, `priority`, `audit`, `silent`, `doc_excepted` keep
their current names and semantics.

## The OR operator

`{"or": [clause, clause, ...]}` is a directive to the **rule loader**,
not the engine. It is expanded to disjunctive normal form before any
rule reaches the engine.

### Parse shape

A clause in the `when` array is either a leaf condition or an OR
group:

```rust
enum WhenClause {
    Leaf(DiskCondition),   // { "new_content_contains": ".unwrap()" }
    Or(Vec<WhenClause>),   // { "or": [ ... ] }
}
```

`DiskRule.when` parses into `Vec<WhenClause>`.

### Unfolding semantics (`unfold_or`)

Runs after parse, before `rule_from_disk`. Converts each `DiskRule`
into `1..N` OR-free `DiskRule`s:

- **No OR** → rule emitted unchanged.
- **One OR, K alternatives**, at position i → K rules, each with the
  OR replaced by alternative k.
- **Multiple ORs in one `when`** → cartesian product.
  `[X, OR(A,B), Y, OR(C,D)]` → 4 rules.
- **Nested OR** `OR(A, OR(B, C))` → flattened to `OR(A, B, C)` via
  recursive expansion.
- **Empty OR** `{"or": []}` → **load-time error**, naming the rule id.
  An unsatisfiable condition is a bug; fail loudly.
- **Single-element OR** `{"or": [a]}` → degenerates to `a`, no
  expansion.

### Generated rule ids

When one rule expands to N, child ids are deterministic from
condition order:

- Single OR: `<id>#or0`, `<id>#or1`, ...
- Multi-OR cartesian: `<id>#or0-or1`, `<id>#or0-or2`, ...

The `#` separator does not collide with the kebab-case id convention
and greps cleanly.

### Expansion bound

K^N (N = OR count in a `when`, K = arity). No hard cap, but a
load-time **warning** when expansion produces >32 rules from one
source, naming the source id.

### Isolation property

The engine never sees OR. `unfold_or` guarantees every rule reaching
`rule_from_disk` has a pure-AND `when`. This keeps the engine, the
hook, and the audit path unchanged — OR lives entirely in the loader.

## Migration strategy

Three composing behaviors:

1. **Parser reads both shapes.** Per-rule detection: a `"conditions"`
   key → v1; a `"when"` key → v2. Mixed files allowed. This is what
   keeps existing projects working the moment the 0.8.0 binary is
   installed.
2. **Writer emits v2 only.** `write_atomic` always produces v2. Files
   walk into v2 the next time anything saves them (`save_rules`,
   `init --force`).
3. **`phr-mcp migrate-rules <path>` for explicit conversion.**

### `migrate-rules` behavior

```
phr-mcp migrate-rules <path>            # convert in place (.json.bak backup)
phr-mcp migrate-rules --dry-run <path>  # print converted JSON, write nothing
phr-mcp migrate-rules --check <path>    # exit 0 if already v2, 1 if v1 (CI gate)
```

- Reads via both-shapes parser, writes v2.
- Preserves OR clauses as written — does **not** expand OR (expansion
  is a load-time concern; on disk, `{"or": [...]}` stays intact).
- Idempotent: re-running on a v2 file rewrites it byte-identically
  (key ordering pinned).
- In-place mode backs up to `.json.bak` via existing `write_atomic`
  logic.

## Load pipeline

```
read()  →  Vec<DiskRule>  →  unfold_or()  →  Vec<DiskRule>  →  rule_from_disk()  →  engine
         (parse, both          (1 → 1..N,       (OR-free)
          shapes)               DNF)
```

`unfold_or` is wired into both load paths:
- `hook::load_rules` (pre/post-check hooks)
- `server_persistence` autoload (live MCP network)

so OR behaves identically at hook time and in the MCP server.

## Internal types

`DiskCondition` (`predicate`/`args`/`script`) and `DiskAction`
(`action_type`/`params`) keep their Rust shape — only their serde
impls change. This preserves the ~10 direct `DiskCondition { ... }`
construction sites in `audit.rs`. The new shape is a wire-format
concern only.

New type: `WhenClause` (above). Lives in `rules_file.rs`.

## Testing strategy

| Layer | Tests |
|-------|-------|
| Parser | v1 parses; v2 parses; mixed-shape parses; malformed errors cleanly |
| Serializer | v2 round-trips byte-stable; v1 never emitted |
| `unfold_or` | no-OR passthrough; 1 OR→N; multi-OR cartesian; nested flatten; empty-OR errors; single-element degenerates; id determinism; >32 warns |
| `migrate-rules` | v1→v2; idempotent on v2; `--dry-run` writes nothing; `--check` exit codes; backup created |
| Integration | hook fires on a v2 rules.json; hook fires on a v1 rules.json (compat); **OR rule matches either branch end-to-end through a real pre-check** |
| Default pack | `init` emits valid v2; every shipped rule parses + unfolds + loads into the engine |

The end-to-end OR integration test is load-bearing: write a rules.json
with `{"or": [A, B]}`, run an actual `phr-mcp pre-check` with a
payload matching only B, assert it fires. Proves the unfold→engine
path, not just unit-level expansion.

One backward-compat regression test is mandatory: a v1-shape file fed
through `read()` must parse and load correctly. Guards the migration
promise.

## Commit plan (4 commits)

1. **`feat: rule schema v2 — when/then/predicate-as-key wire format`**
   — `rules_file.rs` serde rewrite (both-shapes read, v2 write),
   `WhenClause` type, round-trip + compat tests. No behavior change:
   OR not wired, defaults not rewritten.
2. **`feat: OR operator via load-time rule unfolding`** — `unfold_or`
   + tests + end-to-end OR integration test + wiring into both load
   paths.
3. **`feat: phr-mcp migrate-rules command`** — subcommand + tests.
4. **`chore: rewrite default rule pack + test fixtures in v2 shape;
   bump 0.8.0`** — mechanical `init.rs` + fixture rewrites, version
   bump, CLAUDE.md update.

Commits 1-3 each compile and pass tests independently. Commit 4
isolates the bulk-mechanical diff so it doesn't obscure logic changes.

## Rollout

1. Land all 4 commits; `cargo install --path crates/phronesis-mcp` →
   binary is 0.8.0.
2. `phr-mcp migrate-rules ~/Git/phronesis/.phronesis/rules.json` —
   converts this project's 36 rules (incl. the 2 commit-timing
   customs) to v2.
3. `phr-mcp migrate-rules ~/Git/<consumer>/.phronesis/rules.json` —
   converts the consumer's 34 default-pack rules.
4. Both projects work throughout: the both-shapes parser reads their
   existing v1 files even before migration. Migration only makes the
   on-disk shape current.

the consumer's `cargo build` is unaffected — only `phronesis-mcp` changes;
the consumer depends on `phr`, which is untouched.

## Open questions

- **Key ordering in serialized output.** To make `migrate-rules`
  idempotent and diffs clean, the writer must pin key order
  (`id`, `phase`, `priority`, flags, `when`, `then`). `serde_json`
  preserves struct field order, so this falls out of struct field
  ordering — confirm during implementation.
- **`>32` expansion warning channel.** At hook time we cannot print to
  stdout (pollutes the JSON-RPC / hook protocol). The warning should
  go to stderr (hook) or the action log. Decide during implementation.
