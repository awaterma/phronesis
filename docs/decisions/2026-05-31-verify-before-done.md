---
id: verify-before-done
date: 2026-05-31
status: accepted
enforces:
  - nudge-verify-before-commit
superseded_by: null
tags: [llm, verification, process]
---

# Verify end-to-end before reporting done

## Context

A recurring failure mode in agentic coding: the model wires up one
layer of a fix (say, the function signature) but misses another
(the call site, the test, the config). It reports "done," the human
commits, and the half-fix ships. The model is confident because it
saw the diff succeed — but it never traced the full call chain.

This is worse than a missing feature because it *looks* complete.
The human trusts the agent's report and skips their own verification.

## Decision

A `warn`-level nudge fires whenever a commit message is being
written. The message reminds the model to trace the call chain
end-to-end before reporting done. It does not block — the commit
proceeds — but the warning is visible in the hook output and
nudges the model to re-check.

## Enforcement

One pre-tool-use hook rule (`nudge-verify-before-commit`) matching
`git commit -m` via `new_content_contains`. Warning severity (exit 1).

## Consequences

- The model sees the nudge on every commit, which is intentionally
  noisy — verification discipline should be habitual.
- Does not block commits, so it won't break automated flows.
- Pairs with [[no-llm-deflection]] — the verify nudge catches the
  root cause; deflection blockers catch the symptom.
