# Phronesis for Coding Agents

> If you are an autonomous coding agent, Phronesis is a tool you can use to
> understand and check your own work. This page tells you what it does, when
> it is worth trying, how to connect it, and how to test its claims on your
> own repository before relying on it.

This page is for agents **discovering** Phronesis — deciding whether it helps
with a task in some *other* codebase. If you are working *on* Phronesis
itself (building, testing, changing this repository), read
[`AGENTS.md`](../AGENTS.md) instead.

Every command and output excerpt below was run against this repository at
v0.38.0. Where something depends on setup you may not have (coverage
collection, hooks), the page says so.

---

## What Phronesis does

Phronesis is a RETE rules engine (`phr-mcp`) that sits beside a coding agent
and keeps structured, inspectable evidence about the code it is changing:

| Capability | What you get | Where it lives |
|---|---|---|
| **Structural code graph** | Typed relations between files, functions, tests, modules, and rules: `defines_fn`, `calls`, `tested_by`, `test_reaches`, `no_direct_test`, `imports`, `in_cycle`, … Rust, Python, TypeScript, Swift, and Java. | `.phronesis/graph.jsonl` (derived, rebuildable) |
| **Test selection** | Tests relevant to the functions you changed, each labeled by the kind of evidence behind it (static graph reach, or observed per-test coverage). | `phr-mcp coverage select` |
| **Coverage evidence** | Per-test coverage imported at a known revision, so "this test executes this region" is an observation, not a guess. | `.phronesis/coverage/` |
| **Confidence signals** | Build / test / known-bug outcomes parsed from commands you actually ran, rolled into a band for the current work unit. | `phr-mcp confidence` |
| **Rule-based governance** | Rules on disk, evaluated by host hooks at every tool call, that can warn about or block an edit or command. Every decision is logged with the rule that caused it. | `.phronesis/rules.json`, `.phronesis/log.jsonl` |
| **Provenance** | Each consequence records whether it came from a rule firing, a lookup, or an assertion; each block names its rule or the fail-closed stage. | log entries, MCP tool responses |

### Phronesis is not a retrieval system

Phronesis is not primarily a code search engine. It maintains structured
relationships between code, tests, rules, and evidence so you can reason
about a change and evaluate its consequences.

**Use retrieval to find relevant information; use Phronesis to establish and
inspect the evidence supporting a change.** A search tool answers "where is
`parse_config`?". Phronesis answers "which tests reach `parse_config`, on
what evidence, and is that evidence current for the tree on disk?" — and
says so when it cannot answer.

---

## When to consider it

Worth trying when your task involves:

- **Change impact** — who calls the function you are about to change, and
  which tests exercise it.
- **Test selection** — running the tests relevant to a change instead of
  the whole suite, or finding functions with no direct test.
- **Verification of your own work** — recording what you actually built and
  tested, so a completion claim rests on journaled outcomes.
- **Long or multi-session work** where project rules must hold at hour three
  as well as minute one (rules are re-read from disk on every tool call, not
  carried in your context window).
- **Codebase analysis** — module dependencies, import cycles, structural
  hotspots.

Probably not worth it when:

- The task is a one-off lookup that `grep` or your host's search already
  answers.
- The language is not one of the graph's languages (Rust, Python,
  TypeScript/JavaScript, Swift, Java). Rules still work; the graph will not.
- You cannot run a local binary (Phronesis is a CLI + stdio MCP server, not
  a hosted service).

---

## How to connect

### 1. Install the binary

```sh
cargo install phronesis-mcp          # from crates.io; installs `phr-mcp`
# or, from a checkout of this repository:
cargo install --path crates/phronesis-mcp

phr-mcp --version
```

### 2. Choose how much to turn on

There are two levels. **Start with the first.**

**Read-only evaluation (no hooks, nothing that can block you).** The graph
and test-selection commands work in any git repository without `init`:

```sh
cd /your/project
phr-mcp graph rebuild    # writes only .phronesis/graph.* and bindings.json
```

This writes only derived files under `.phronesis/`; `phr-mcp state` lists
them and `rm -rf .phronesis` removes them (`phr-mcp clean --cache` removes
the graph caches but keeps the directory).

**Governed mode (hooks + MCP server).** `phr-mcp init` writes host hook
configuration that runs on **every tool call** and can **block your edits
and commands** when a rule fires:

```sh
phr-mcp init --dry-run           # see exactly what would be written
phr-mcp init                     # language-neutral platform
phr-mcp init --packs rust        # ... plus a language pack (python, typescript, swift, …)
```

It writes `.claude/settings.local.json`, `.mcp.json`, `.gemini/settings.json`,
`.codex/hooks.json`, `.codex/config.toml`, starter rules under `.phronesis/`,
and `.gitignore` entries. Because this changes how your host behaves for
every future session in that project, **ask the human you work for before
running `init`** — and before `phr-mcp install`, which writes user-scope
config (`~/.claude.json`, `~/.gemini/settings.json`).

### 3. MCP configuration

`phr-mcp serve` is a stdio MCP server. `init` registers it per project; you
can also add it by hand.

Claude Code (`.mcp.json`) and Gemini CLI (`.gemini/settings.json`):

```json
{
  "mcpServers": {
    "phronesis": { "command": "phr-mcp", "args": ["serve"] }
  }
}
```

Codex CLI (`.codex/config.toml`):

```toml
[mcp_servers.phronesis]
command = "phr-mcp"
args = ["serve"]
```

Supported hosts for the hooks: **Claude Code**, **Codex CLI**, and **Gemini
CLI**. Any MCP client can use the server's tools. Host trust gates apply:
Codex silently skips new or changed project hooks until they are trusted via
`/hooks`, and Gemini skips project hooks until the folder is trusted.

---

## What to try first

Each step has a CLI form and, where one exists, the MCP tool that does the
same thing.

| Step | CLI | MCP tool |
|---|---|---|
| Build the graph | `phr-mcp graph rebuild` | `rebuild_code_graph` |
| Is it current? | `phr-mcp graph status` | `get_code_graph_status` |
| What relations exist? | `phr-mcp graph query` | `query_code_graph` (no `relation`) |
| Tests for a function | `phr-mcp graph query tested_by my_fn` | `query_code_graph` `{"relation":"tested_by","args":["my_fn"]}` |
| Callers of a function | `phr-mcp graph query calls '*' my_fn` | `query_code_graph` `{"relation":"calls","args":["*","my_fn"]}` |
| Tests for your uncommitted change | `phr-mcp coverage select` | — |
| Untested functions | `phr-mcp graph query no_direct_test` | `query_code_graph` `{"relation":"no_direct_test"}` |
| Whole-tree rule audit | `phr-mcp audit` | `audit_codebase` |
| Confidence for the work unit | `phr-mcp confidence` | `get_confidence` |

**Query syntax.** A query is a relation followed by positional arguments.
`*` alone means "any value"; `*` and `?` inside an argument are globs. Node
IDs are fully qualified (`rust:<crate>::<module>::<Type>::<fn>`). An argument
with no glob matches an ID exactly or as a trailing suffix, so
`fire_all_consequences` and `ReteNetwork::fire_all_consequences` both match
`rust:phronesis::network::ReteNetwork::fire_all_consequences`, and
`phronesis::wme` matches `rust:phronesis::wme`. The suffix must start right
after a `:` (a `::` separator or the `rust:` prefix), so `all_consequences`
matches nothing. Use globs
(`'*network*'`, `'rust:phronesis::*'`) for anything else. Add `--json` for
machine-readable output and `--limit 0` for all rows; the total is always
reported so truncation is visible. Over MCP, a query that returns zero rows
comes back with diagnostics suggesting relations and ID formats to try.

---

## What evidence to expect

Real output from this repository.

**Relation vocabulary** (`phr-mcp graph query`, excerpt):

```
Relation             Edges
test_reaches         59936
tested_by             4460
calls                 4035
defines_fn            3142
no_direct_test        1742
imports                617
in_cycle                 6
```

**Direct tests of a function** (`graph query tested_by fire_all_consequences --limit 3`):

```
tested_by  rust:phronesis::network::ReteNetwork::fire_all_consequences  rust:phronesis-mcp#test:journey_derive::absence_via_zero_count_fires
tested_by  rust:phronesis::network::ReteNetwork::fire_all_consequences  rust:phronesis::network::fire_all_consequences_tests::fire_all_consequences_empty_when_no_agenda
tested_by  rust:phronesis::network::ReteNetwork::fire_all_consequences  rust:phronesis::network::fire_all_consequences_tests::fire_all_consequences_returns_one_per_fired_action

3 of 32 match(es); raise --limit for more.
```

**Callers** (`graph query calls '*' fire_all_consequences --limit 3`):

```
calls  rust:phronesis-mcp#bench:journey_derive::bench_journey_full  rust:phronesis::network::ReteNetwork::fire_all_consequences
calls  rust:phronesis-mcp::codex_hook::fire_verdict                 rust:phronesis::network::ReteNetwork::fire_all_consequences
calls  rust:phronesis-mcp::context::capsule::matched                rust:phronesis::network::ReteNetwork::fire_all_consequences

3 of 9 match(es); raise --limit for more.
```

**JSON shape** (`--json`): every row says whether it was extracted from
source or derived by a rule.

```json
{"results":[{"args":["rust:phronesis-mcp::action_log::LogEntry::new","rust:phronesis-mcp::action_log::tests::entry"],"derived":false,"relation":"tested_by"}],"returned":1,"total":4460}
```

**Test selection for an uncommitted edit.** After a one-line edit inside
`fire_all_consequences`, `coverage select` first refused to use the graph:

```
No tests selected: the coverage store is empty or no hits match changed regions.
  static reach: graph stale (1 file(s) drifted); static reach omitted. Run `phr-mcp graph rebuild`
```

After `graph rebuild`, it named the changed function and the tests that
statically reach it, each labeled with its evidence kind:

```json
{"evidence": "static_reach", "regions": ["fn:crates/phronesis/src/network.rs::ReteNetwork::fire_all_consequences"], "test": "rust:phronesis#test:perf_smoke::hot_path_completes_within_budget"}
```

It selected 613 tests — `fire_all_consequences` is the engine's main entry
point, and static reach is transitive. With per-test coverage imported
(`phr-mcp coverage collect`, or `coverage import` for Python, Java, Swift,
JS/TS, and Lua), tests that were *observed* hitting the changed region are
listed separately under `coverage_observation`, which is usually far
narrower.

**Resolution honesty.** `graph rebuild` and `graph status` report calls the
resolver could not attribute, per file:

```
Resolution hotspots (unresolved then ambiguous):
  crates/phronesis-mcp/tests/hook_integration.rs: 1042 unresolved, 0 ambiguous
  crates/phronesis-mcp/src/graph/extract.rs: 780 unresolved, 11 ambiguous
```

Unresolved and ambiguous calls are **dropped, not guessed**. Read this list
before trusting an empty `calls` result.

---

## Evaluate Phronesis on your own repository

Do not take the claims above on trust. Test them on a real task.

1. **Pick a function changed in a recent commit** (`git log -p -5`).
2. **Ask Phronesis for its callers and tests:**
   ```sh
   phr-mcp graph rebuild
   phr-mcp graph query calls '*' <fn> --limit 0
   phr-mcp graph query tested_by <fn> --limit 0
   ```
3. **Compare with your usual workflow** — grep, your host's code search, your
   language server's "find references", the project's test conventions.
   Note what each finds that the other misses.
4. **Make a small edit to the function, then select and run tests:**
   ```sh
   phr-mcp graph rebuild
   phr-mcp coverage select --json
   ```
   Run the selected tests. Did any fail that you would not have run? Did a
   test you know matters go unselected? Revert the edit afterwards.
5. **Check the limits it reported** — the resolution hotspots, `graph
   status`, and any `static_note` / `coverage_note` in the JSON.
6. **Report discrepancies** — missing relationships, wrong attributions,
   claims the evidence does not support — as an issue at
   <https://github.com/awaterma/phronesis/issues>, with the language, the
   query, the expected result, and the actual result.

You do not need to believe Phronesis is useful. Establish whether it helps
with a real task, and report what you find either way.

### Evaluate governance on a scratch repository

The graph is the least distinctive part. This checks the rest: a rule on
disk decides a tool call, and the log records which rule and why. Use a
throwaway repository, so `init` needs no permission to change the human's
project. In a real host the hook runs automatically on every tool call; the
pipe below only lets you see the decision directly.

1. **Set up:**
   ```sh
   mkdir -p /tmp/demo/src && cd /tmp/demo && git init -q
   printf 'pub fn parse(s: &str) -> Option<i32> {\n    s.parse().ok()\n}\n' > src/lib.rs
   git add -A && git commit -qm init
   phr-mcp init --packs rust   # 52 rules, incl. enforce-no-unwrap-in-src (phase "pre")
   ```
2. **Send the hook a Claude Code `PreToolUse` payload** for an edit that adds
   `.unwrap()`:
   ```sh
   echo '{"session_id":"demo","hook_event_name":"PreToolUse","cwd":"/tmp/demo",
     "tool_name":"Edit","tool_input":{"file_path":"/tmp/demo/src/lib.rs",
     "old_string":"    s.parse().ok()","new_string":"    Some(s.parse().unwrap())"}}' \
     | phr-mcp pre-check; echo "exit=$?"
   ```
   ```
   phronesis: BLOCKED — Avoid .unwrap() in src/ — use ? for error propagation, or expect() with a clear message if truly unreachable.
   exit=2
   ```
   The same payload with `"new_string":"    s.trim().parse().ok()"` prints
   `{}` and exits 0.
3. **Read why** in `.phronesis/log.jsonl` (trimmed):
   ```json
   {"kind":"hook","event":"pre_check","tool":"Edit","file":"/tmp/demo/src/lib.rs","exit":2,
    "blocked_by":[{"kind":"rule","rule":"enforce-no-unwrap-in-src","message":"Avoid .unwrap() in src/ — …"}],
    "consequences":[{"rule_id":"enforce-no-unwrap-in-src","action_type":"constraint_violation","message":"…"}]}
   ```
   and `phr-mcp stats`:
   ```
   Rule                      Blocked  Warned  Last fired
   enforce-no-unwrap-in-src        1       0  3s ago
   ```
4. **Change the rule on disk and repeat step 2.** Set that rule's `"phase"`
   to `"none"` in `.phronesis/rules.json`: the same payload exits 0. Restore
   `"pre"`: it exits 2 again. Nothing is restarted; every hook call re-reads
   the file. A typo such as `"phase": "Pre"` fails closed — exit 2 with
   `blocked_by: [{"kind":"fail_closed","stage":"load_rules", …}]` naming the
   rule, field, and allowed values.

Editing `rules.json` also counts as a change to the graph's inputs: until the
next rebuild the hook prints a note that the graph is stale and structural rules will
warn rather than block; `phr-mcp graph rebuild` clears it. The `.unwrap()`
block above still applied. Remove the scratch repository when done.

---

## What it does not establish

- **It does not prove code correct.** The graph is extracted statically from
  syntax plus name resolution; `tested_by` means "a test calls this", not
  "a test checks this behavior".
- **Missing edges are possible.** Dynamic dispatch, macros, reflection,
  generated code, and unresolved imports can hide calls. Unresolved calls
  are dropped, so an empty answer can mean "unknown" rather than "none".
  Check `graph status` and the hotspot list.
- **Static reach over-selects.** `test_reaches` is transitive; for central
  functions it can select most of the suite. Observed coverage narrows it,
  but only after you collect and import it — and only at the revision it was
  collected for (hits from another revision are reported as stale).
- **The graph reflects the last rebuild.** Queries (`graph query`,
  `query_code_graph`) answer from whatever was last built; check
  `graph status` / `get_code_graph_status` first. Test selection
  (`coverage select`) refuses a stale graph outright. After `git checkout`,
  `git mv`, a rebase, or edits made outside a hooked host, run
  `phr-mcp graph rebuild`.
- **Confidence reflects only what was recorded.** The band comes from
  build/test outcomes the hooks journaled or that you recorded with
  `phr-mcp signal`; it says nothing about checks that were never run.
- **Rules are only as good as their patterns.** Most starter rules are
  lexical or structural matches; a quiet audit means no rule matched, not
  that the code is good. Drift reports are heuristic leads for review.
- **Formal verification is narrow.** The Verus harness under
  `crates/phronesis-mcp/verification/` proves invariants of Phronesis's own
  coverage store. It does not verify your code.

---

## Further reading

- [README](../README.md) — overview and subsystems
- [Command reference](../crates/phronesis-mcp/CLAUDE.md) — every CLI command and hook
- [Explainer](https://awaterma.github.io/phronesis/explainer.html) — the engine and design intent
- [Rule catalogue](https://awaterma.github.io/phronesis/catalogue.html) — starter rules and their rationale
- [SPEC-coverage-evidence](specs/SPEC-coverage-evidence.md) — how coverage evidence and test selection work
- [SPEC-confidence-scoring](specs/SPEC-confidence-scoring.md) — how confidence bands are derived
