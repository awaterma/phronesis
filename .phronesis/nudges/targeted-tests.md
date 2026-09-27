---json
{
  "id": "targeted-tests-for-changes",
  "priority": 70,
  "max_bytes": 512,
  "when": {
    "predicate": "journey_seen",
    "args": ["source-edit", "s"]
  }
}
---

Source changed this session. Before claiming it works, run
`phr-mcp coverage select` (it reads unstaged edits only: run it before
`git add`) and run every test it lists. A changed function it lists no test
for needs a new test. If it says the graph is stale, run
`phr-mcp graph rebuild` first.
