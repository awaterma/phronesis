---
id: test-driven-features-and-bugs
date: 2026-11-23
status: accepted
enforces:
  - warn-tdd-before-source-change
superseded_by: null
tags: [testing, workflow, governance]
---

# Test-driven development for features and bug fixes

## Context

Passing tests written after implementation do not establish that those tests
would have caught the original defect or missing behavior. The collaborator
requested test-driven development for both features and bug fixes, with a
rule and a contextual reminder to keep the practice visible during a session.

## Decision

Write a focused behavioral or regression test first. Run it and observe a
failure caused by the intended missing behavior, then implement the smallest
passing change and refactor while tests remain green. Keep red/green evidence.
A missing API may first fail compilation; behavioral regressions should fail
on an assertion about the defect. An unrelated build failure is not a valid
red result. Disclose if implementation preceded the test: verifying a later
test against the original code is useful regression evidence, not retroactive
TDD.

## Enforcement

The pre-hook rule `warn-tdd-before-source-change` emits a warning for
Rust source paths (`file_path_matches: src`, `file_extension_is: rs`). It does
not block edits or claim to verify the order of test and implementation work.
These predicates inspect path components and extensions, not source text.

The capsule `test-driven-features-and-bugs` uses the allowlisted
`journey_seen(source-edit, s)` predicate. Creation and edits recorded by the
host hooks tag source files, including tests; the capsule then reminds the
agent at the next context event in the same session. It cannot pre-announce
an unobserved edit or prove tests were run. The pre-hook rule and the standing
AGENTS.md/durable directive supply the earlier reminder.

Reviewed presets live in `config/phronesis/rules.json`,
`config/phronesis/journey.json`, and `config/phronesis/nudges/tdd.md`.
`phr-mcp provision` installs missing files into the local `.phronesis/`
directory, preserving customized files and reporting conflicts. Existing
projects must merge the rule and taggers rather than replace their rule set.
This change does not automatically install policy in other projects. Only
observed hook calls can trigger the capsule.

## Consequences

The process requirement is explicit and the reminder is re-injected, but
review still checks relevant red/green evidence. Hooks do not treat any
failing test as proof that TDD was followed. Documentation-only changes do
not trigger the source reminder. Full source-to-test chronology enforcement
would need additional trustworthy evidence and is not claimed here.
