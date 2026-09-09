# Java implementation: Qwen review and verification

Reviewer: `Inferact/Qwen3.8-27B-NVFP4` at the user-requested Spark-980a endpoint.
The user explicitly authorized transfer of the current spec, Java source,
relevant integration changes, and corpus evidence. The initial bundle included
31 files plus diffs (91,365 input tokens). An analysis-only response exhausted
its output limit; the subsequent direct-report request returned six proposed
blockers. These are static reviewer inferences, not executed tests.

## Findings and disposition

| Finding | Local verification | Disposition |
|---|---|---|
| 1. List-valued Bazel `select` in deps/exports is skipped | `review_fixtures_keep_list_selects_and_reject_scalar_list_attributes` verifies both dependencies and exported targets are visible, zero unsupported statements, and two select counters | Not reproduced: list branches return `Value::List`; scalar `StringChoices` is a separate case |
| 2. Bazel should accept scalar strings in list-valued attributes | The same fixture rejects scalar `srcs` with a diagnostic; Bazel declares native Java `srcs`, `deps`, and `exports` as lists of labels | Proposed normalization would accept an invalid native rule; retain typed attribute handling |
| 3–4. External-parent coordinates must not supply Maven group/root interpolation | `declared_external_parent_coordinates_supply_group_and_root_interpolation` verifies the group literally declared in the local POM's parent element, while recording the unavailable parent and importing none of its build configuration | Retain coordinate-only behavior; clarify it explicitly in the spec |
| 5. Mixed scalar/list selects are not rejected | Existing code rejects differing branch categories; the reviewer fixture produces one unsupported statement and unclaimed sources | Not reproduced; regression now pins the exact fixture |
| 6. Module-import handling can accept parser errors | The supplied example is valid Java 25 and intentionally supported, but refined fixtures `import module class;` and `import module _;` exposed acceptance of reserved words | Fixed: reject reserved words, boolean/null literals, and single underscore, while preserving contextual words and valid module imports |

Bazel's attribute types are documented in its
[Java rule reference](https://bazel.build/reference/be/java). Maven's parent
coordinate declaration is described by its
[model reference](https://maven.apache.org/ref/3.9.15/maven-model/maven.html).
The identifier exclusions follow
[JLS 25 sections 3.8–3.9](https://docs.oracle.com/javase/specs/jls/se25/html/jls-3.html#jls-3.8).

The parser regression failed before the fix: `import module class; class A {}`
produced a nonfailed source with a module import and declaration. After the
fix, all 56 Java tests pass, including valid contextual-name module imports,
reserved/literal-name rejection, and the Bazel/Maven reviewer fixtures.
The declaration-cache format was incremented from 1 to 2 so persisted
pre-fix parses cannot retain the incorrect evidence.

## Follow-up verification

Qwen received the actual relevant source, test fixtures, before/after results,
and semantic references for independent reassessment (16,389 input tokens).
It returned `VERDICT: PASS` and withdrew all six proposed blockers. Its
reassessment is a static opinion based on supplied evidence, not execution
of our tests. Some explanatory wording remains imprecise (mixed select
branches are rejected, rather than converted to lists); the source and local
regressions are the authority for those details.

Final validation after the parser fix passed:

- `cargo test --workspace --tests --examples`: 2,487 Rust tests and 46 BDD scenarios.
- `cargo clippy --workspace --tests --examples -- -D warnings`.
- `cargo build --workspace` and `cargo fmt --all -- --check`.
- Both real corpus rebuilds produce graphs byte-identical to the pre-review
  baselines and regenerate declaration caches with format 2.

A proposed blocker was not accepted merely because a model named it, and
the PASS verdict did not replace local validation. No code was committed,
merged, published, or deployed as part of this review.
