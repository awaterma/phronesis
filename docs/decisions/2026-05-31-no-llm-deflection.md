---
id: no-llm-deflection
date: 2026-05-31
status: accepted
enforces:
  - enforce-no-pre-existing-issue
  - enforce-no-not-from-our-changes
  - enforce-no-not-caused-by-our
  - enforce-no-should-work-claim
  - enforce-no-should-be-fixed-claim
superseded_by: null
tags: [llm, deflection, accountability]
---

# Block LLM deflection disclaimers

## Context

LLMs have a recurring failure mode where they disclaim ownership of
problems in the code they just touched. Phrases that shift blame
or assert correctness without evidence erode trust — the human reads
them as weasel words and has to manually verify anyway.

The problem compounds in agentic loops: an unverified completion
claim gets accepted by the outer loop, the bug ships, and nobody
notices until production.

## Decision

Block five deflection patterns at the pre-tool-use hook. Each
pattern matches a specific blame-shifting or unverified-claim
substring:

1. Disclaimers citing prior state of the code — either fix the
   issue as part of this change or defer with a clear rationale.
2. Disclaimers attributing the problem to other changes — name the
   issue and decide: fix or defer.
3. Disclaimers denying causal responsibility — own the fix or own
   the deferral.
4. Unverified claims that a fix "will work" — run the verification
   before claiming completion, or label the work "untested."
5. Unverified claims that a problem "is resolved" — verify
   end-to-end or mark untested.

All five are `block` severity — the hook exits non-zero and the
model sees the message. The model must rephrase to proceed.

## Enforcement

Five pre-tool-use hook rules, each matching a specific substring in
the model's output via `new_content_contains`. Blocking (exit 2).

## Consequences

- The model learns to either verify or say "untested" — both are
  honest signals the human can act on.
- Occasional false positives when the phrases appear in quoted text
  or documentation about the rules themselves. The model rephrases
  and moves on; low friction.
- Pairs with [[verify-before-done]] — the deflection blockers
  catch the symptom, the verify nudge catches the root cause.
