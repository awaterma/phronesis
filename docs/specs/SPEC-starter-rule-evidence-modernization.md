# SPEC: Starter-rule evidence modernization

**Status:** proposed
**Date:** 2026-08-19
**Scope:** `llm` and `rust` starter packs
**Related:** `SPEC-confidence-scoring.md`,
`SPEC-non-code-span-masking.md`, `SPEC-call-graph-and-suppression-rules.md`,
`SPEC-extensible-predicates.md`

## 0. Summary

Ten long-lived starter rules currently mix three different kinds of evidence
behind the same priority-10 blocking presentation:

1. exact prose fragments used as proxies for deflection or unsupported
   completion claims;
2. raw source substrings used as proxies for Rust call sites; and
3. a structural Rust predicate for `Result<_, String>` return types.

The rules encode useful policy, but their identifiers and severity make their
evidence look more uniform than it is. This spec modernizes their evidence
without weakening pre-edit enforcement or pretending that Phronesis can infer
human intent from prose.

The target is not “replace every substring with the graph.” A pre-check sees
the proposed payload before the edited code is present in the persistent code
graph. A graph-only replacement can therefore miss the exact new call that a
pre rule is meant to block. The target is:

- hook-time structural facts for proposed Rust content;
- persistent graph facts for post-check explanation and whole-tree audit;
- grounded outcome facts for completion gates;
- explicitly labelled lexical heuristics where no semantic fact exists; and
- a shadow comparison before any old trigger is retired.

## 1. Rules in scope

### 1.1 LLM/reporting pack

- `enforce-no-not-caused-by-our`
- `enforce-no-not-from-our-changes`
- `enforce-no-pre-existing-issue`
- `enforce-no-should-be-fixed-claim`
- `enforce-no-should-work-claim`

### 1.2 Rust pack

- `enforce-no-panic-in-src`
- `enforce-no-result-string-error`
- `enforce-no-todo-in-src`
- `enforce-no-unimplemented-in-src`
- `enforce-no-unwrap-in-src`

This is a starter-pack design. Downstream copies of these rules are not
silently rewritten.

## 2. Current evidence and its limits

| Rule family | Current evidence | Useful property | Limit |
|---|---|---|---|
| Deflection | `new_content_contains` on three phrases | Cheap and deterministic | Paraphrases evade it; quotation or policy discussion can trigger it |
| Completion claims | `new_content_contains` on two phrases | Catches a recurring handoff failure | Does not establish whether verification happened and misses other wording |
| Panic/TODO/unwrap | substring plus `file_path_matches("src")` | Operates on proposed content during pre-check | Text is not a call site; path matching is not production-code classification |
| String errors | `function_returns_result_string(?file, ?fn)` | Structural and function-specific | Scope and audit behavior are not stated explicitly by the rule |

These are evidence limits, not proof that the policies are wrong. The update
must preserve the policy/evidence distinction in rule metadata, catalogue
output, and diagnostics.

There is also an observation limit: `new_content_contains` evaluates content
carried by a governed hook event. It does not inspect arbitrary assistant prose
unless the host routes that prose through such an event. The current phrase
rules therefore cannot honestly be described as universal output gates.

## 3. Requirements

### R1. No pre-check regression

A proposed production Rust edit introducing `unwrap`, `panic`, `todo`, or
`unimplemented` must still be blockable before the filesystem mutation occurs.
Persistent graph facts alone do not satisfy this requirement.

### R2. Structural Rust call evidence

The hook must expose a neutral fact for API-family calls parsed from the
proposed Rust content:

```text
function_calls_api(file, function, api)
```

`api` initially supports the same normalized vocabulary already used by graph
`calls_api`: `unwrap`, `expect`, `panic`, `todo`, and `unimplemented`.

The hook-time extractor and graph extractor must share normalization tests so
the same syntax does not acquire two names depending on clock. This does not
require making the transient hook fact persistent.

### R3. Explicit production scope

Structural blocker rules use a production-code classification rather than a
substring search for `src`. Test modules and test files remain exempt according
to the Rust pack's documented policy. The implementation must define behavior
for examples, benches, build scripts, generated code, and nested `src` paths.

If the existing `file_type(file, "production")` fact is unavailable on the
pre-check clock, the hook must emit an equivalent neutral classification fact
or make `file_type` available there. The rule must not join a transient
proposed-content fact to an unrelated stale file identity.

### R4. Graph-backed explanation and audit

Post-check diagnostics and whole-tree audit may use persistent:

```text
defines_fn(file, function)
calls_api(function, api)
file_type(file, "production")
```

Graph evidence must be reported with the normal closed-world and freshness
limits. An empty result is not proof that a call does not exist.

### R5. Preserve `Result<_, String>` as structural evidence

`function_returns_result_string(file, function)` remains the authoritative
predicate. The starter rule must document whether it applies to all parsed
Rust functions or only edited production functions. The recommended starter
policy is edited production functions at hook time, with whole-tree debt
available through audit.

### R6. Completion claims use grounded outcomes where available

The two completion-claim rules must not treat wording as proof of missing
verification. When confidence is enabled and an open subject exists, the gate
uses the subject's grounded compile, test, and known-bug signals.

The minimum interim behavior is:

- a matching lexical completion phrase plus insufficient grounded signals may
  block;
- a matching phrase with sufficient grounded signals must not be described as
  “unverified” solely because of the phrase; and
- absence of a phrase must not be described as proof that a completion claim
  is supported.

The preferred long-term vocabulary is:

```text
completion_claim(subject)
verification_sufficient(subject)
```

`verification_sufficient` can be host-derived from confidence outcomes.
`completion_claim` is deferred until Phronesis has a deterministic host event
or an explicit agent protocol for declaring a done-claim. It must not be
implemented by presenting a phrase classifier as semantic certainty.

### R7. Deflection rules remain lexical and say so

No current fact establishes that prose is blame-shifting, that an issue was
deferred with rationale, or that an agent “owned” a decision. The three
deflection rules therefore remain lexical heuristics in this release.

Their catalogue entries and diagnostics must label them as heuristic phrase
matches. Rule IDs remain stable initially so downstream stats and governance
links do not break. Renaming is a separate compatibility decision, not part of
the evidence upgrade.

### R8. Evidence kind is inspectable

Catalogue and rule inspection output should distinguish at least:

- `lexical` — text or regex evidence;
- `syntactic` — parser-derived hook evidence;
- `structural` — persistent graph/whole-tree evidence; and
- `outcome` — observed build, test, or known-bug evidence.

This may be explicit rule metadata or derived presentation metadata. It must
round-trip through save/load if explicit. This spec does not require the RETE
engine to assign epistemic weight to the categories.

### R9. Observation boundaries are explicit

Rule inspection and catalogue output must state which host events can supply a
rule's facts. A future `completion_claim` rule requires an explicit governed
event or agent protocol; adding the predicate without a producer would create
the appearance of enforcement while matching nothing.

## 4. Proposed rule shapes

### 4.1 Hook-time Rust blocker

Conceptually:

```json
{
  "id": "enforce-no-unwrap-in-src",
  "phase": "pre",
  "priority": 10,
  "evidence_kind": "syntactic",
  "when": [
    {"edited_file": "?file"},
    {"file_type": ["?file", "production"]},
    {"function_calls_api": ["?file", "?fn", "unwrap"]}
  ],
  "then": {
    "block": "`?fn` in ?file calls unwrap; propagate the error or document an invariant with expect."
  }
}
```

Exact v2 rule syntax is an implementation detail. The required joins and
semantics are not.

Equivalent rules cover `panic`, `todo`, and `unimplemented`. They do not add
`no_direct_test`: the current starter policy is an unconditional production
block, and test coverage would change that policy rather than improve its
evidence.

### 4.2 Graph audit rule

The whole-tree form joins production files and normalized graph calls. It may
share the public rule ID if hook and audit backends can select different fact
sources without ambiguity; otherwise it uses an internal companion ID hidden
from starter-pack users while preserving one reported policy identity.

### 4.3 Completion phrase heuristic

Until an explicit completion-claim event exists, keep phrase matching as a
candidate detector and join it to confidence state when available. Do not
write a rule whose only condition is “two signals exist” unless the configured
confidence policy says two are sufficient; the current confidence spec treats
three out of three as high.

### 4.4 Deflection heuristic

Keep the three current patterns and stable IDs, but render diagnostics as:

> Heuristic phrase match: this wording can deflect responsibility. Name the
> issue and state whether it will be fixed or deferred, with rationale.

The message describes what the evidence supports. It does not claim to have
inferred intent.

## 5. Migration

### Phase A — instrument

1. Add hook-time `function_calls_api` extraction and production classification.
2. Add unit fixtures for all supported call families and non-code spans.
3. Expose evidence kind in inspection/catalogue output.
4. Do not change starter-rule behavior yet.

### Phase B — shadow comparison

Run lexical and structural/syntactic detectors together and log, without an
additional user-facing warning:

```text
rule_evidence_comparison(rule_id, lexical, syntactic, structural)
```

Measure at least:

- lexical-only matches;
- syntactic-only matches;
- agreement;
- parser unavailable/partial cases; and
- pre/post graph-generation mismatches.

The comparison is triage evidence, not automatic proof that either detector is
correct. Review representative mismatches before changing enforcement.

### Phase C — switch authority

Make hook-time syntactic evidence authoritative for Rust pre-checks only after
the acceptance corpus passes. Keep a lexical fallback when parsing is
unavailable or partial, and identify fallback evidence in the diagnostic.

Use graph evidence for post-check explanation and whole-tree audit. Preserve
stable public rule IDs and existing pack membership.

### Phase D — outcome-aware reporting

Join completion-phrase candidates to confidence outcomes. Keep the deflection
rules lexical and labelled. Defer semantic completion/deferral facts until an
explicit protocol exists.

## 6. Acceptance tests

### 6.1 Rust pre-check

For each of `unwrap`, `panic`, `todo`, and `unimplemented`:

1. a proposed call in a production function blocks before write;
2. the same token in a comment, string, or documentation example does not
   produce syntactic evidence;
3. a test function and a test file follow the documented exemption;
4. multiline and qualified spellings normalize correctly where Rust permits
   them;
5. a partial or invalid edit cannot silently bypass policy—the lexical fallback
   fires and identifies itself as fallback evidence; and
6. post-check/audit identifies the containing function and file after graph
   refresh.

### 6.2 String error

1. `Result<T, String>` in an edited production function blocks;
2. aliases and qualified `std::result::Result<T, String>` have an explicit,
   tested policy;
3. test-only code follows the documented exemption; and
4. audit can report existing debt without requiring a new edit.

### 6.3 Reporting

1. each legacy phrase still produces a candidate match;
2. catalogue/inspection output labels phrase rules lexical/heuristic;
3. a completion phrase with insufficient confidence is blocked with the
   missing grounded signals named;
4. sufficient confidence does not yield an “unverified” diagnostic solely from
   the phrase; and
5. paraphrases are documented as out of scope rather than represented as
   covered.

### 6.4 Compatibility

1. `phr-mcp init --rules-only --force --packs llm,rust` preserves public rule
   IDs and pack selection behavior;
2. saved v2 rules round-trip any new evidence metadata;
3. stats, debt trend, drift, and decision bindings retain continuity by rule
   ID; and
4. projects without a fresh graph retain pre-check protection.

## 7. Non-goals

- inferring moral responsibility or author intent from prose;
- using an LLM classifier inside deterministic hook enforcement;
- replacing confidence policy with phrase matching;
- weakening unconditional Rust production blockers based on test coverage;
- silently rewriting downstream project rule files; or
- treating graph absence as proof of code absence.

## 8. Open decisions

1. Should evidence kind be stored in the rule schema or derived from its
   predicates for presentation?
2. Should hook-time API-call facts reuse the name `calls_api`, or use
   `function_calls_api` to make their transient source explicit?
3. How should partial parse status be surfaced to rules: a fact, provenance on
   emitted facts, or a host-selected lexical fallback?
4. Can one public rule cleanly own separate pre-check and audit fact sources,
   or are internal companion rules required?
5. What explicit agent event, if any, should assert `completion_claim` without
   relying on natural-language classification?
6. Should hosts expose an explicit “handoff/done claim” event, or should
   Phronesis continue gating only observable actions such as commit?

## 9. Implementation touchpoints

Expected areas, to be confirmed against the code at implementation time:

- Rust hook syntax extraction under `crates/phronesis-mcp/src/syntax/rust/`;
- graph API-call normalization under `crates/phronesis-mcp/src/graph/`;
- hook fact assembly and parse-status handling;
- starter packs in `crates/phronesis-mcp/src/init.rs`;
- v2 rule schema and catalogue/inspection rendering if evidence metadata is
  explicit;
- confidence fact derivation and commit/done-claim gating; and
- integration fixtures for Claude Code and Codex pre/post payloads.

The exact modules are intentionally not contractual. The acceptance behavior
and evidence boundaries are.
