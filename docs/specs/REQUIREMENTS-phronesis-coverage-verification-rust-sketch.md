# Phronesis: Coverage + Verification Evidence Graph (Rust Sketch)

> **Status (2026-09-23):** Retained as the vision document. Decomposed for
> implementation into three specs, grounded in an engine-capability audit of
> `crates/phronesis` and `crates/phronesis-mcp`:
>
> - **A** — `SPEC-coverage-evidence.md`: dynamic coverage store, changed-region
>   facts, relevant-test derivation, evidence gaps
> - **B** — `SPEC-property-ontology.md`: properties as first-class entities,
>   provenance, promotion lifecycle, proof signals
> - **C** — `SPEC-verification-artifact-generation.md`: safety-gated artifact
>   generation (deferred until A + B land)
>
> Engine constraints that reshape this sketch: no pattern-level negation
> (host-derived closed-world facts instead), no set-valued rule heads
> (host-side selection query instead), in-memory working memory (durable
> on-disk evidence store instead), and the sketch's "Phronesis-Rye"
> layer is `phronesis-rhai` (Rhai template scripts render artifact
> bodies; the sandbox cannot write files or run verifiers, so the host
> does the write and the execution).
> All sections below are preserved unchanged as the conceptual record.

## Goal

Show how Phronesis can combine:

- **per-test code coverage**
- **formal verification evidence**
- **change impact**
- **rule-driven reasoning**

so that an agent can answer:

> Which tests exercise the code I changed, which properties are formally verified, and where are the remaining evidence gaps?

---

## 1. Tiny Rust library

```rust
pub fn safe_divide(numerator: i32, denominator: i32) -> Result<i32, &'static str> {
    if denominator == 0 {
        return Err("division by zero");
    }

    Ok(numerator / denominator)
}
```

Imagine the library has three tests:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn divides_positive_values() {
        assert_eq!(safe_divide(8, 2), Ok(4));
    }

    #[test]
    fn divides_negative_values() {
        assert_eq!(safe_divide(-8, 2), Ok(-4));
    }

    #[test]
    fn rejects_zero_denominator() {
        assert_eq!(safe_divide(8, 0), Err("division by zero"));
    }
}
```

The interesting question is not merely:

> What percentage of `safe_divide` is covered?

It is:

> **Which tests provide evidence for which behavior?**

---

## 2. Capture per-test execution evidence

Run tests separately (or instrument them so test identity is retained) and collect coverage at function / region / branch granularity.

Conceptually Phronesis receives facts like:

```text
test("divides_positive_values")
test("divides_negative_values")
test("rejects_zero_denominator")

function("safe_divide")

test_hits_function(
  test="divides_positive_values",
  function="safe_divide"
)

test_hits_function(
  test="divides_negative_values",
  function="safe_divide"
)

test_hits_function(
  test="rejects_zero_denominator",
  function="safe_divide"
)

test_hits_branch(
  test="rejects_zero_denominator",
  function="safe_divide",
  branch="denominator == 0"
)
```

You can make this more precise with source regions:

```text
test_hits_region(
  test="rejects_zero_denominator",
  file="src/lib.rs",
  start_line=3,
  end_line=5
)
```

This gives Phronesis an edge:

```text
test -> executed region -> function
```

rather than just a global coverage percentage.

---

## 3. Add formal verification evidence

Now imagine `safe_divide` has properties expressed for a verifier such as Verus.

At the Phronesis level, you do not need to bake Verus syntax into the core ontology. Store the semantic result:

```text
verification_property(
  id="safe_divide.nonzero_returns_quotient",
  function="safe_divide"
)

verification_property(
  id="safe_divide.zero_returns_error",
  function="safe_divide"
)

verification_result(
  property="safe_divide.nonzero_returns_quotient",
  verifier="verus",
  status="proved"
)

verification_result(
  property="safe_divide.zero_returns_error",
  verifier="verus",
  status="proved"
)
```

Now there is another path:

```text
function -> property -> proof evidence
```

---

## 4. Record a code change

Suppose an agent modifies the zero-denominator branch:

```diff
 if denominator == 0 {
-    return Err("division by zero");
+    return Err("invalid denominator");
 }
```

Phronesis records:

```text
changed_region(
  change="commit_abc123",
  file="src/lib.rs",
  start_line=3,
  end_line=5
)
```

and derives:

```text
changed_function(
  change="commit_abc123",
  function="safe_divide"
)
```

---

## 5. Infer relevant tests

A simple rule:

```text
IF
  changed_region(Change, Region)
AND
  test_hits_region(Test, Region)

THEN
  relevant_test(Change, Test)
```

For this change:

```text
relevant_test(
  change="commit_abc123",
  test="rejects_zero_denominator"
)
```

The positive and negative division tests may still transitively touch the function, but they do **not** exercise the changed branch.

That distinction is valuable:

```text
affected_test     != test_that_calls_function
branch_relevant   != function_relevant
```

---

## 6. Infer which proofs must be rerun

Associate properties with code regions or semantic entities:

```text
property_depends_on_region(
  property="safe_divide.zero_returns_error",
  file="src/lib.rs",
  start_line=3,
  end_line=5
)
```

Rule:

```text
IF
  changed_region(Change, Region)
AND
  property_depends_on_region(Property, Region)

THEN
  verification_required(Change, Property)
```

Derived result:

```text
verification_required(
  change="commit_abc123",
  property="safe_divide.zero_returns_error"
)
```

Now the change has two immediate evidence requirements:

```text
commit_abc123
   |
   +--> run test: rejects_zero_denominator
   |
   +--> reprove: safe_divide.zero_returns_error
```

---

## 7. Detect evidence gaps

This is where Phronesis gets more interesting than a coverage dashboard.

Rule:

```text
IF
  changed_region(Change, Region)
AND NOT
  exists test_hits_region(Test, Region)
AND NOT
  exists proved_property_covering(Region)

THEN
  evidence_gap(Change, Region)
```

Instead of:

> Coverage fell from 87.4% to 86.9%.

you can report:

> **The changed behavior has neither dynamic test evidence nor formal proof evidence.**

That is much closer to an engineering risk statement.

---

## 8. Minimal test selection

Once Phronesis has the coverage graph, test selection becomes derivable.

For a change set:

```text
changed_region -> tests_that_hit_region
```

Take the union:

```text
minimal_relevant_test_set(Change)
```

For example:

```text
minimal_relevant_test_set(
  change="commit_abc123",
  tests=["rejects_zero_denominator"]
)
```

A policy can then decide:

```text
IF change_risk == "low"
THEN run minimal_relevant_test_set

IF change_risk >= "medium"
THEN run relevant_tests + package_tests

IF release_candidate
THEN run full_suite
```

Phronesis is not replacing the full test suite. It is providing **evidence-directed test selection**.

---

## 9. Evidence graph

Conceptually:

```text
                           +----------------------+
                           |  formal property P1  |
                           +----------+-----------+
                                      |
                                      v
                                [proved: Verus]
                                      ^
                                      |
+---------+     executes      +-------+-------+
| Test A  | ----------------> |               |
+---------+                   |               |
                              | safe_divide() | <---- changed code
+---------+     executes      |               |
| Test B  | ----------------> |               |
+---------+                   +-------+-------+
                                      ^
                                      |
+---------+   branch coverage         |
| Test C  | --------------------------+
+---------+
```

The useful Phronesis query becomes:

> **What evidence supports this changed behavior?**

Possible answer:

```text
Change: commit_abc123

Changed behavior:
  safe_divide / zero-denominator branch

Dynamic evidence:
  rejects_zero_denominator
    - executes changed branch
    - last passed: current run

Formal evidence:
  safe_divide.zero_returns_error
    - verifier: Verus
    - previous result: proved
    - status: stale because dependent code changed
    - action: reprove

Evidence gaps:
  none
```

---

## 10. A useful Phronesis ontology

A small initial vocabulary could be:

```text
code_entity
function
source_region
branch
test
property
change

test_hits_function
test_hits_region
test_hits_branch

property_applies_to_function
property_depends_on_region

verification_result
verification_required

changed_region
changed_function

relevant_test
evidence_gap
```

This can stay language-neutral.

Adapters supply facts from:

```text
Rust   -> llvm coverage / cargo tooling + Verus or Kani
Java   -> JaCoCo + OpenJML
Python -> coverage.py + Nagini
```

The RETE layer reasons over one common evidence model.

---

## 11. Agent workflow

A coding agent changes a library:

```text
1. Agent edits code
2. Change detector emits changed_region facts
3. Phronesis joins changes against historical per-test coverage
4. Phronesis derives relevant_test facts
5. Phronesis joins changes against verification dependencies
6. Phronesis derives verification_required facts
7. Agent runs selected tests and verifier
8. Results are appended as provenance
9. Phronesis evaluates remaining evidence gaps
10. Policy decides whether the change is acceptable or escalation is required
```

That makes test execution and verification part of a single **evidence lifecycle**.

---

## 12. Why this is useful

Traditional CI tends to keep these separate:

```text
coverage report
test report
static analysis
formal verification
change diff
```

Phronesis can turn them into a connected evidence graph:

```text
change
  -> code
      -> tests
      -> properties
      -> proof results
      -> provenance
```

The resulting question is no longer:

> Did CI turn green?

It becomes:

> **What evidence justifies believing this change is correct, and what evidence is missing?**

That is a much stronger basis for autonomous-agent governance.

---

## 13. Properties as first-class semantic entities

The earlier sketch treated proof properties as metadata attached to functions. This discussion strengthens that idea: a **property should be a first-class entity in Phronesis**.

The verifier-specific syntax is not the property itself. Verus `requires` / `ensures`, JML annotations, Nagini contracts, Kani harnesses, and generated tests are all **encodings or evidence mechanisms** for a language-neutral semantic property.

Example:

```text
property(
  id="safe_divide.zero_returns_error",
  subject="safe_divide",
  kind="postcondition",
  condition="denominator == 0",
  guarantee="result is Error"
)
```

This creates three distinct layers:

```text
semantic property
      ↓
verifier/test encoding
      ↓
verification evidence
```

That separation is important because Phronesis can reason about the stable meaning of the property even when different tools encode it differently.

---

## 14. Verifier adapters

Each verifier becomes an adapter from a normalized property into a tool-specific artifact.

Conceptually:

```text
Property
   |
   +--> Rust + Verus     -> requires/ensures/spec function
   |
   +--> Rust + Kani      -> proof harness
   |
   +--> Java + OpenJML   -> JML contract
   |
   +--> Python + Nagini  -> contract/specification
```

Phronesis should record the relationship explicitly:

```text
property_encoding(
  property="safe_divide.zero_returns_error",
  language="rust",
  verifier="kani",
  artifact="verification/safe_divide_zero.rs"
)
```

and:

```text
verification_result(
  property="safe_divide.zero_returns_error",
  verifier="kani",
  evidence_kind="bounded_model_check",
  status="passed",
  source_revision="abc123"
)
```

Do not flatten all verifier outcomes into a single `verified=true` flag. Different tools provide different kinds and strengths of evidence.

Possible evidence kinds:

```text
deductive_proof
bounded_model_check
runtime_unit_test
property_based_test
integration_test
static_analysis
coverage_observation
```

---

## 15. Generate verification artifacts from properties

Once properties are first-class, Phronesis can derive executable verification obligations.

A normalized property:

```text
property:
  id: safe_divide.zero_returns_error
  subject: safe_divide
  condition: denominator == 0
  guarantee: result is Error
```

could generate a Kani proof harness:

```rust
#[kani::proof]
fn zero_denominator_is_error() {
    let n: i32 = kani::any();
    assert!(safe_divide(n, 0).is_err());
}
```

and a conventional Rust test:

```rust
#[test]
fn zero_denominator_is_error() {
    assert!(safe_divide(8, 0).is_err());
}
```

The generated artifacts are different, but they are both derived from the same semantic property.

That gives Phronesis a more general relationship:

```text
property
   ↓ derives
verification_obligation
   ↓ rendered-by
tool_template
   ↓ produces
verification_artifact
   ↓ produces
evidence
```

---

## 16. Rye as the verification artifact generator

Phronesis-Rye can naturally serve as the template/transformation layer.

A Rye rule or template can select on:

```text
language
verifier
property_kind
evidence_policy
```

For example:

```text
rust + kani + postcondition
    -> kani-postcondition.rhai

rust + verus + postcondition
    -> verus-postcondition.rhai

java + openjml + postcondition
    -> openjml-postcondition.rhai

python + nagini + postcondition
    -> nagini-postcondition.rhai
```

The overall flow becomes:

```text
Phronesis property
      ↓
verification_required
      ↓
Rye selects template
      ↓
Rye renders verifier artifact
      ↓
verifier executes
      ↓
result appended to provenance log
      ↓
Phronesis updates evidence graph
```

This keeps verifier-specific concerns out of the RETE core.

---

## 17. Property provenance and confidence

A critical constraint: **observed behavior must not automatically become intended behavior**.

If logs repeatedly show:

```text
invalid input -> returns null
```

that observation is useful evidence, but it is not yet equivalent to:

```text
property:
  invalid input MUST return null
```

The repeated behavior might be accidental or buggy.

Therefore every property should carry provenance such as:

```text
property_source:
  explicit_spec
  existing_verifier_contract
  test_assertion
  documentation
  code_inference
  runtime_observation
  agent_inference
```

and a lifecycle/status such as:

```text
observed
candidate
corroborated
accepted
verified
rejected
superseded
```

Example:

```text
property_candidate(
  id="safe_divide.zero_returns_error",
  source="runtime_observation",
  confidence=0.55
)
```

A policy can determine when automatic generation is permitted:

```text
IF property.source == "explicit_spec"
THEN verification_generation_allowed

IF property.source == "runtime_observation"
AND corroborating_sources >= 2
THEN verification_generation_allowed

IF property.source == "runtime_observation"
AND corroborating_sources == 0
THEN mark_as_candidate_only
```

The exact confidence model can remain policy-driven; the important point is that **Phronesis preserves how the property was learned**.

---

## 18. Deriving candidate properties from logs

Logging can become an input to property discovery.

Useful logged facts may include:

```text
test assertions
test executions
function calls
branches exercised
error outcomes
agent decisions
code changes
existing specs
existing verification results
runtime observations
```

Phronesis can infer candidate properties:

```text
runtime observations
        ↓
behavioral pattern
        ↓
candidate property
        ↓
corroboration / review / policy
        ↓
accepted property
        ↓
generated verification artifact
```

For example:

```text
observed(
  subject="safe_divide",
  condition="denominator == 0",
  outcome="Error",
  count=487
)
```

could derive:

```text
candidate_property(
  subject="safe_divide",
  condition="denominator == 0",
  guarantee="result is Error",
  source="runtime_observation"
)
```

but **not** immediately promote it to a normative requirement.

This lets Phronesis automate property discovery without enshrining accidental implementation behavior.

---

## 19. Closed-loop verification automation

The complete system now looks like:

```text
code / tests / logs / specs
          ↓
      fact ingestion
          ↓
      Phronesis RETE
          ↓
   property discovery
          ↓
 candidate / accepted property
          ↓
verification obligation
          ↓
       Rye template
          ↓
 generated verifier artifact
          ↓
 verifier execution
          ↓
 structured result
          ↓
 append-only provenance
          ↓
      Phronesis RETE
          ↓
 evidence sufficient?
```

For a coding-agent change:

```text
1. Agent edits code.
2. Phronesis identifies changed regions/functions.
3. Existing properties whose dependencies intersect the change become stale.
4. Historical per-test coverage identifies relevant runtime tests.
5. Accepted properties create verification obligations.
6. Rye generates missing or refreshed verifier artifacts.
7. Tests and verifiers execute.
8. Results are logged with source revision and tool metadata.
9. Phronesis updates the evidence graph.
10. Rules identify remaining evidence gaps.
11. Policy decides whether the agent can proceed autonomously or requires review.
```

This turns verification from a separate CI stage into a **self-updating evidence loop**.

---

## 20. Revised evidence graph

The earlier evidence graph should now be extended to include semantic properties and generated artifacts:

```text
                    Requirement / Spec
                           |
                           v
                    Semantic Property
                    /      |       \
                   /       |        \
          encoded-as   encoded-as   derives-test
             /             |             \
        Verus spec      Kani harness      Unit/property test
             \             |             /
              \            |            /
               +------ Evidence -------+
                          |
                          v
                    source revision
                          ^
                          |
Test coverage ---> Code Region <--- Change
      |                 |
      +---- executes ---+
```

The key idea is:

> **Properties are the semantic join point between intent, code, tests, proof tools, and provenance.**

---

## 21. New or revised Phronesis entities

The ontology should expand beyond the initial sketch:

```text
code_entity
function
source_region
branch
test
change

property
property_candidate
property_source
property_status
property_dependency

verification_obligation
property_encoding
verification_artifact
verification_result
evidence_kind

test_hits_function
test_hits_region
test_hits_branch

changed_region
changed_function

relevant_test
verification_required
stale_evidence
evidence_gap
```

The important distinction is between:

```text
property        = semantic claim
encoding        = tool-specific expression of claim
artifact        = generated file/harness/spec
result          = verifier execution outcome
evidence        = what the result contributes
```

---

## 22. Impact on the original design

This discussion changes the original sketch in several useful ways.

### Before

The initial model was primarily:

```text
change
  -> coverage
  -> relevant tests
  -> existing formal properties
  -> rerun / reprove
```

### Now

The stronger model is:

```text
logs/specs/tests/code
        ↓
discover or import properties
        ↓
reason about provenance/confidence
        ↓
generate verification artifacts
        ↓
run proofs/tests
        ↓
record evidence
        ↓
invalidate/recompute evidence after changes
```

So Phronesis is no longer only **consuming verification results**.

It can also coordinate the lifecycle of verification:

```text
discover
normalize
generate
execute
record
invalidate
regenerate
reason
```

That is a materially broader role.

---

## 23. Suggested implementation boundary

A clean implementation split would be:

### Phronesis core

Owns:

```text
property ontology
property provenance/status
coverage relationships
change relationships
verification obligations
evidence relationships
RETE rules
policy decisions
```

### Phronesis-Rye

Owns:

```text
property -> tool-specific artifact generation
templates
language-specific rendering
verifier invocation recipes
result normalization helpers
```

### Verifier adapters

Own:

```text
Verus
Kani
OpenJML
Nagini
future verifier integrations
```

### Coverage adapters

Own:

```text
Rust LLVM coverage
Java JaCoCo
Python coverage.py
future language-specific collectors
```

This keeps the core language-neutral while letting Rye handle the intentionally language/tool-specific generation layer.

---

## 24. Stronger statement of the concept

The central concept is now:

> **Phronesis maintains a provenance-aware graph of software properties and the evidence supporting them. It can infer candidate properties from observed development/runtime evidence, promote them according to policy, generate verifier-specific proof or test artifacts through Rye, execute those checks, and automatically invalidate or regenerate evidence when code changes.**

The practical result is not merely smarter test selection or formal verification.

It is a system that can answer:

> **What do we claim should be true about this code, why do we think that claim is intended, what evidence currently supports it, and what must be rerun or regenerated because of this change?**

