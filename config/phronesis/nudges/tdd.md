---json
{
  "id": "test-driven-features-and-bugs",
  "priority": 80,
  "max_bytes": 512,
  "when": {"predicate": "journey_seen", "args": ["source-edit", "s"]}
}
---

Use TDD for features and bug fixes: write a focused behavioral or regression
test first, run it and observe the intended failure, make the smallest change
to pass, then refactor while tests stay green. Preserve red/green evidence.
If implementation preceded the test, say so; retrospective regression checks
are not test-first development. This source-change reminder does not prove
that TDD was followed.
