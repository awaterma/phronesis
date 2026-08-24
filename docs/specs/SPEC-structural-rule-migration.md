# Structural starter rules and advisory confidence gates

Status: Implemented

## Summary

Phronesis starter packs still contain rules that infer code structure from
`new_content_contains` substrings. Those rules are vulnerable to matches in
comments and string literals, formatting changes, aliases, and syntactically
different forms of the same construct. Replace them with syntax-tree facts
where the repository has a supported parser and the policy is genuinely about
program structure.

At the same time, make missing build or test evidence advisory at Git mutation
time. A repository with incomplete confidence evidence should warn, not block.
Known failing evidence may continue to produce a stronger warning, but this
feature must not prevent the Git command.

This is an incremental migration, not a promise to eliminate regular
expressions. Natural-language policy, unsupported languages, and shell-command
recognition remain lexical until a more appropriate producer exists.

## Motivation

The current rules mix three different kinds of evidence under
`new_content_contains`:

1. program structure, such as Rust macro calls, attributes, parameter types,
   match arms, and method calls;
2. command intent, such as `git commit`, `git add -A`, or process-control
   commands;
3. prose, such as unverified-completion and deflection phrases.

Only the first category should be represented by language syntax facts. The
second already has the command-scoped `bash_command_matches` predicate and
should use it consistently. The third is inherently textual and must not be
misrepresented as structural.

The confidence starter rules currently block Git mutations at low confidence
(`<= 1` passing grounded signal), warn at medium confidence (`== 2`), and pass
at high confidence. This makes absence of recorded evidence an enforcement
failure even when the user wants to commit work in progress. The desired policy
is observability without refusal.

## Goals

- Replace code-shape substring conditions with facts emitted from the parsed
  syntax tree wherever a supported grammar can express the policy reliably.
- Preserve or improve rule precision without silently reducing coverage.
- Keep hook evaluation and whole-tree audit behavior consistent.
- Make incomplete build/test confidence advisory for Git commands.
- Preserve stable rule IDs where the policy is unchanged, so logs, decisions,
  statistics, and local overrides remain meaningful.
- Document which lexical rules intentionally remain lexical and why.

## Non-goals

- Eliminating all regular expressions.
- Parsing shell into a complete POSIX/Bash AST in this change.
- Adding a Rhai parser solely to replace the `print(` rule.
- Converting prose-governance rules into syntax predicates.
- Promoting any warning rule to a blocker.
- Changing confidence signal derivation or the definition of a work unit.

## Terminology and classification

- **Structural fact:** a fact produced from a language parser node and carrying
  enough identity to explain the finding, normally file, enclosing item, and
  construct.
- **Command-scoped lexical fact:** a match against the normalized Bash command,
  never against edited file contents or arbitrary tool payload text.
- **Textual rule:** a rule intentionally matching prose or a language for which
  no parser-backed fact exists.

The migration inventory must classify every packaged `new_content_contains`
condition into exactly one of these categories before code changes begin.

## Proposed behavior

### 1. Structural predicate contract

New syntax facts must follow the existing `SyntaxFacts` model:

- the predicate is listed in `SyntaxFacts::PREDICATES`;
- `SyntaxFacts::all_facts` is the single conversion point to RETE facts;
- the fact includes `?file` and, where meaningful, `?fn` or another enclosing
  item plus the construct value;
- extraction is values-aware and ignores comments and string literals;
- malformed or partially edited input does not panic;
- unsupported extensions emit no such fact;
- identical findings have deterministic ordering and stable arguments.

Prefer reusable facts over one predicate per starter rule when a stable syntax
concept exists. Examples include a Rust macro invocation, attribute, method
call, implementation target, parameter type, or match-arm shape. Use a
specialized predicate when the policy requires semantic analysis that cannot
be represented safely by a generic node fact.

Do not call a substring search performed inside a tree-sitter node
"structural." Node kinds and named fields must identify the construct; source
text may only be used to extract the value of that already-identified node.

### 2. Migration tiers

The inventory should be executed in the following order.

#### Tier A: replace with existing facts

First migrate rules already covered by current producers, including any rule
whose intended construct is represented by predicates such as
`engine_eval_string_literal`, `rust_async_blocking_call`,
`rust_sync_lock_guard_across_await`, `rust_unsafe_without_safety_comment`,
Python structural predicates, TypeScript structural predicates, and Swift
force-unwrap facts.

If a packaged rule and an existing predicate differ in scope, arguments, or
threshold, add equivalence fixtures before deciding whether the fact can be
reused. Similar names are not proof of equivalent behavior.

#### Tier B: add focused parser-backed facts

Add facts for high-value Rust starter rules that currently recognize code
shape lexically. The implementation inventory should evaluate at least:

- panic-like macro invocations: `unwrap`, `expect`, `panic!`, `todo!`,
  `unimplemented!`, and `dbg!`;
- crate/item attributes such as `deny(warnings)` and `allow(dead_code)`;
- public parameter types such as `&Box<T>` and ID fields using primitive
  `String`/`u64` types;
- `impl Deref` declarations;
- empty or swallowing `match` arms;
- `Rc<RefCell<_>>` type shapes;
- `std::env::set_var` calls;
- string concatenation policy, only if the exact intended expression can be
  stated and tested structurally.

This list is an evaluation queue, not automatic approval. A rule stays lexical
if its current meaning is ambiguous, its proposed structural form would change
policy, or tree-sitter recovery makes the hook-time false-negative rate
unacceptable.

After Rust, evaluate remaining Swift and TypeScript code-shape rules against
their existing parsers. Add facts only when both hook-time edits and audit-time
full files can be tested through real entry points.

#### Tier C: intentionally lexical rules

Keep the following lexical, with an inline rationale near the pack definition:

- deflection and unverified-completion phrases;
- `.rhai` source rules until a Rhai parser-backed producer exists;
- shell command patterns, using `bash_command_matches` rather than
  `new_content_contains`;
- languages or constructs without a supported parser;
- compatibility fixtures whose purpose is to test the lexical predicate.

### 3. Compatibility and rollout

- Keep migrated starter rule IDs and messages unless the policy itself changes.
- Generated rules from `phr-mcp init --rules-only --force` must contain the new
  conditions. Existing project rule files are not rewritten automatically by
  this feature.
- If automatic migration is desired later, specify it separately with versioned
  input/output examples and collision behavior.
- Update packaged-rule documentation and ADR enforcement references that name
  `new_content_contains` when their rule becomes structural.
- Code-graph bindings that deliberately inspect `new_content_contains` must
  treat structural rules as non-literal rather than manufacturing a source
  binding. Any user-visible freshness or coverage consequence must be tested.

### 4. Confidence gate severity

Change the low-confidence Git gate from `block` to `warn`. The resulting matrix
is:

| Evidence for the open work unit | Result | Exit behavior |
| --- | --- | --- |
| Build and tests are both grounded green, with no known-bug failure | no confidence warning | allow |
| Build or tests have not been run or are not grounded | warning identifying the missing evidence | warn, do not block |
| A grounded build/test/known-bug signal is failing | warning identifying failure, stronger wording permitted | warn, do not block |

The current implementation counts aggregate `signal_pass` facts. Before
changing the rule, verify that count bands can distinguish "missing" from
"failed." If they cannot, either:

1. keep truthful aggregate wording (for example, "confidence evidence is
   incomplete or failing"), or
2. add explicit status facts in a separately reviewed change.

Do not claim which signal is missing unless the asserted facts prove it.

By default, preserve the existing command scope
`git (commit|merge|rebase|cherry-pick|revert|pull)` and change severity only.
Although the request is commonly described as the "git commit blocker," the
current rule governs all of those mutations. Narrowing it to commit alone is a
separate policy decision and would otherwise silently weaken existing coverage.

Both low and medium bands may remain separate warning rules for distinct
messages and statistics. No confidence-band consequence may have action type
`constraint_violation` after this change.

## Correctness requirements

### Predicate producer/consumer seams

For every new predicate:

- prove the extractor emits it for positive syntax and not for a comment,
  string literal, similarly named symbol, or neighboring construct;
- prove `all_facts` maps it with the documented argument order;
- prove the hook asserts it for supported file edits;
- prove `phr-mcp audit` evaluates it on full files;
- prove a real packaged rule consumes it and produces the expected consequence;
- include malformed/partial source coverage appropriate for editor-time hooks.

### Semantic-equivalence fixtures

Before replacing a lexical condition, create a fixture table containing:

- canonical positive syntax;
- whitespace and multiline variants;
- qualified, aliased, or turbofish variants where meaningful;
- comment and string-literal negatives;
- test/example/source path scope;
- malformed partial-edit behavior;
- at least one case the regex got wrong and the structural predicate fixes.

Record intentional behavior changes in the test name or adjacent comment. A
structural migration is not required to preserve known regex false positives.
It must not introduce an unexplained false negative for valid target syntax.

### Confidence-gate tests

Drive `pre-check` and `post-check` through the binary, as the existing
`confidence_gate_integration` tests do. Cover:

- no evidence;
- build only;
- tests only;
- both build and tests green;
- known failing build;
- known failing tests;
- confidence disabled;
- each governed Git command or a table-driven assertion over the shared matcher;
- warning exit/output and explicit absence of exit code 2.

Update assertions and test names that currently require low confidence to
block. Retain a test proving unrelated blocking rules still return exit code 2.

## Implementation plan

1. **Freeze the inventory.** Generate a checked review table of all packaged
   `new_content_contains` uses: rule ID, pack, language, category, current
   scope, proposed predicate, and disposition. Exclude tests whose purpose is
   the generic lexical engine, but do not exclude generated starter rules.
2. **Change the confidence policy independently.** Convert the low band to a
   warning, correct its wording, update the confidence spec/ADR/docs, and run
   focused integration tests. Keeping this diff separate makes the deliberate
   enforcement change easy to review.
3. **Migrate Tier A.** Rewire rules to existing facts one rule family at a
   time, adding end-to-end regression tests for any previously untested seam.
4. **Implement Tier B by language and construct family.** Add extractor facts,
   fact conversion, packaged consumers, hook/audit tests, and docs together.
   Do not land orphan producers or consumers.
5. **Annotate Tier C.** Replace command uses of `new_content_contains` with
   `bash_command_matches`; document why prose and unsupported-language rules
   remain lexical.
6. **Regenerate and compare starter packs.** Exercise `init` for each affected
   pack and inspect the generated `rules.json`, including merge/force behavior.
7. **Run quality gates.** `cargo fmt --all`, focused syntax/init/hook/audit and
   confidence tests, `cargo test --workspace`, and
   `cargo clippy --workspace -- -D warnings`.
8. **Manual audit.** Run the affected packs against a small fixture repository
   containing positive syntax, comments, strings, and formatting variants;
   compare old and new findings and record explained deltas.

## Suggested change boundaries

For reviewability, prefer multiple pull requests or commits:

1. advisory confidence gates;
2. inventory plus existing-fact migrations;
3. Rust macro/call structural facts;
4. Rust type/attribute/match structural facts;
5. Swift/TypeScript migrations and intentional-lexical documentation.

Each boundary must pass the workspace gates independently. Later groups may be
split further if their extractor behavior is not cohesive.

## Acceptance criteria

- The inventory accounts for every packaged `new_content_contains` condition.
- Every migrated rule has a live parser-backed producer, a packaged consumer,
  hook coverage, audit coverage, and false-positive regression tests.
- Comments and string literals do not trigger migrated code-shape rules.
- Unsupported or malformed source fails safely and predictably.
- Generated starter packs use structural conditions for all approved
  migrations and retain intentional lexical rules with rationale.
- Low or medium confidence never blocks the governed Git commands.
- Confidence messages make only claims supported by recorded evidence.
- Existing project rules are not silently rewritten.
- Formatting, focused tests, workspace tests, and clippy all pass.

## Open decisions for review

1. Should the first implementation milestone cover Rust only, or include Swift
   and TypeScript facts in the same release?
2. Should failing build/test evidence remain a warning, as specified here, or
   should explicit failure still block while merely missing evidence warns?
3. Should the existing broad Git mutation scope be preserved, or should the
   advisory gate narrow to `git commit` only?
4. Is a future versioned migration of existing `.phronesis/rules.json` files
   desirable, or is regeneration via `init --rules-only --force` sufficient?

## Evidence at proposal time

- Repository HEAD is the `v0.30.0` release commit and matches `origin/main`.
- The working tree was clean before this spec was added.
- A fresh `cargo test --workspace` baseline completed successfully on
  2026-08-21, including unit, integration, and documentation tests.

These observations establish a useful baseline but do not prove the proposed
implementation correct.
