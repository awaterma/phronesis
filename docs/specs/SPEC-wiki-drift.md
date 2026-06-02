# SPEC: wiki-drift — decisions → rules extractor

**Status:** draft
**Authors:** Claude, Andrew Waterman
**Date:** 2026-05-29
**Target release:** phronesis-mcp 0.9.0 (MINOR — new CLI subcommand, new MCP tool, new file surface)
**Affects:** `crates/phronesis-mcp/src/{init.rs, main.rs, server.rs, server_params.rs}`,
              new `crates/phronesis-mcp/src/wiki_drift.rs`, new `crates/phronesis-mcp/src/wiki.rs`.
              No `phr` library-crate change.

## Premise

Phronesis already has two drift detectors that flag gaps between
prose guidance and enforced rules:

- `claude-md-drift` — imperatives in `CLAUDE.md` without a matching
  rule. Heuristic (token overlap), no LLM.
- `memory-drift` — auto-memory entries without matching rule or
  durable.md paragraph. Heuristic, no LLM.

Both work, but their source material is unstructured prose. Teams
write decisions all the time — "we will avoid X going forward," "we
adopted Y in May" — and those decisions slowly fade from CLAUDE.md
as it grows, and they were never structured enough to extract
reliably.

This SPEC adds a third detector: **`wiki-drift`**, which extracts
decisions from a structured ADR-style corpus and flags ones that
should be enforced as rules. Source material is markdown with a
frontmatter contract, so matching can use the frontmatter (when
authors are explicit) as well as Jaccard token overlap (when they
aren't).

Bundled with the extractor is the minimal **wiki scaffold** needed
to give it a corpus: a `.phronesis/wiki/decisions/` directory, an
`init`-time README explaining the convention, and a small `decision
new <slug>` helper to scaffold pages from a template.

## Goals

- A structured ADR-style corpus under `.phronesis/wiki/decisions/`
  that humans and LLMs can author and read, that git versions
  (visible project knowledge), and that the extractor can scan.
- `phr-mcp wiki-drift` CLI command + matching `get_wiki_drift` MCP
  tool — heuristic, no LLM, output is a triage list with optional
  `--suggest` draft rule JSON.
- `phr-mcp decision new <slug>` helper that writes a templated ADR
  page operators fill in.
- A small `phr-mcp init` extension that creates the empty wiki
  directory + a README and removes the directory from the
  `.phronesis/` gitignore (the wiki, unlike `rules.json` and
  `log.jsonl`, is meant to be tracked).

## Out of scope (future SPECs)

- **`wiki/consequences/`** — engine-written event pages (Architecture
  B from the exploratory thread). The decisions corpus is sufficient
  for the extractor; consequences pages can come later.
- **`wiki/facts/` and `wiki/rules/`** — fact and rule pages as the
  authoring surface (Architecture A/D). The current `rules.json` and
  in-memory facts stay as they are.
- **Inductive rule mining from consequences.** Explicitly rejected
  in the prior exploration as high-risk for low value.
- **`fact → rule` template library** (#2 from the exploration) —
  separate, smaller, later.
- **LLM-assisted matching/suggestion.** Heuristic only in v1, matching
  the discipline of the existing drift tools.

## The wiki scaffold

### Layout

```
.phronesis/
└── wiki/
    └── decisions/
        ├── README.md                              # what this dir is, conventions
        └── YYYY-MM-DD-<slug>.md                   # one file per decision
```

Filename convention: `<date>-<slug>.md` (e.g. `2026-05-29-card-game-terminology.md`).
Date-prefix sort gives chronological order on filesystem listing.

### Gitignore

`.phronesis/` is project state that, with one exception, should not
be tracked. Decisions are that exception — project knowledge that
*must* be versioned so other contributors and later sessions can
reference them. `phr-mcp init` extends the project's `.gitignore`
(idempotently) with:

```
.phronesis/*
!.phronesis/wiki/
!.phronesis/wiki/**
```

Note the trailing `*` on the broad-ignore line. Plain `.phronesis/`
(no `*`) tells git not to descend into the directory at all, which
makes the `!.phronesis/wiki/` un-ignore inert (verified empirically).
`.phronesis/*` ignores the contents while letting the un-ignore
carve `wiki/` back in. Existing specific entries (`log.jsonl`,
`rules.json.bak`, etc.) remain as belt-and-braces redundancy.

### Decision page schema

```markdown
---
id: card-game-terminology
date: 2026-05-29
status: accepted
enforces:
  - rule-id-or-empty
superseded_by: null
tags: [vocabulary, prose]
---

# Card-game terminology

## Context
What observations or constraints led here?

## Decision
We will use card-game vocabulary throughout the project (hand, card,
member). The RPG vocabulary (party, player, DM, dungeon, etc.) is
removed.

## Enforcement
- (none yet — drift candidate)

## Consequences
- Existing rules referring to "player" should be reviewed.
- Documentation prose follows the new vocabulary.
```

Frontmatter fields:

| Field | Type | Required | Purpose |
|-------|------|----------|---------|
| `id` | kebab-case string | yes | Stable identifier independent of filename |
| `date` | ISO date | yes | When the decision was made |
| `status` | `proposed` \| `accepted` \| `superseded` | yes | Lifecycle |
| `enforces` | list of rule ids (kebab-case strings) | no | Explicit link to rules that enforce this decision |
| `superseded_by` | decision id or `null` | no | Forward pointer when this decision is replaced |
| `tags` | list of strings | no | Free-form categorization |

Body sections are conventional (Context / Decision / Enforcement /
Consequences) but the extractor only reads the Decision and
Enforcement sections — the others are for humans.

## The extractor algorithm

`wiki_drift::run(project_root) -> Result<WikiDriftReport, WikiDriftError>`:

1. **Read wiki directory.** Walk `.phronesis/wiki/decisions/*.md`,
   ignoring `README.md` and anything without frontmatter. Parse each
   into a `Decision { id, date, status, enforces, body }`.
2. **Read the rule pack.** `rules_file::read(default_path)` (already
   returns flat DiskRules with v2 wire format → engine).
3. **Score each decision** against the rule pack:
   - **If `enforces:` lists a rule id that exists in `rules.json` →
     `status = covered`, no further matching needed.** This is the
     explicit-link path — authoritative, deterministic, zero false
     positives.
   - **Else, fuzzy match.** Tokenize the Decision section body
     against each rule's `(id + condition.args + action.params)` blob.
     Same Jaccard machinery as `claude_md_drift`. Best similarity
     above threshold (default 0.15) → `status = likely_covered` with
     a `best_match: rule_id`. Below threshold → `status = uncovered`.
   - **Superseded decisions** (`status: superseded`) are excluded
     from drift scoring — they're history, not active guidance.
4. **Report.** A `WikiDriftReport` lists every active decision with
   its bucket and (where applicable) the matched rule. The CLI
   renders it as a table or JSON.

Output buckets (mirrors memory-drift):

- `covered` — explicit `enforces:` entry that matches an existing rule
- `likely_covered` — fuzzy match above threshold (operator should
  consider adding an explicit `enforces:` link)
- `uncovered` — no match. **Drift candidate.**

## CLI surface

```
phr-mcp wiki-drift                      # table; default wiki dir
phr-mcp wiki-drift --wiki-dir <path>    # override
phr-mcp wiki-drift --json               # machine-readable
phr-mcp wiki-drift --suggest            # emit draft rule JSON for uncovered, on stderr
phr-mcp decision new <slug>             # scaffold a new ADR page
```

`decision new <slug>` creates `.phronesis/wiki/decisions/<today>-<slug>.md`
with the templated frontmatter + skeleton sections. Refuses to
overwrite an existing file.

## MCP tool surface

Mirrors `get_claude_md_drift` / `get_memory_drift`:

```rust
#[tool(description = "Detect drift between ADR-style decision documents in .phronesis/wiki/decisions/ and the current rule pack. ...")]
async fn get_wiki_drift(
    &self,
    Parameters(params): Parameters<GetWikiDriftParams>,
) -> Result<CallToolResult, McpError>;
```

`GetWikiDriftParams`: `format: Option<String>` (json/table), `wiki_dir:
Option<String>`.

## Module layout

| File | Responsibility |
|------|---------------|
| `src/wiki.rs` (new) | Wiki directory paths, page-parsing primitives, `Decision` struct, frontmatter parser. Shared with future wiki SPECs. |
| `src/wiki_drift.rs` (new) | The extractor: `run`, `Decision`, `DriftReport`, `render_table`, `render_json`, `suggest_rule`. Mirrors `claude_md_drift.rs`. |
| `src/init.rs` (modified) | Create `.phronesis/wiki/decisions/` + README at init; add gitignore un-ignore rule. |
| `src/main.rs` (modified) | Add `WikiDrift` + `DecisionNew` subcommands. |
| `src/server.rs` (modified) | Register `get_wiki_drift` MCP tool. |
| `src/server_params.rs` (modified) | `GetWikiDriftParams` struct. |

`wiki.rs` is a deliberate split from `wiki_drift.rs`: future SPECs
(consequences, facts) reuse the page-parsing primitives without
inheriting the drift detector's concerns.

## Testing strategy

| Layer | Tests |
|-------|-------|
| Frontmatter parser | Required fields present / absent; bad YAML errors clearly; tags list / scalar; superseded marker |
| `Decision` extraction | Body-only files (no FM) skipped; README skipped; ordering by date |
| Drift scoring | Explicit `enforces:` shortcut beats fuzzy match; fuzzy threshold honored; superseded decisions excluded; empty-rules-pack edge case |
| `decision new` | Creates file with right filename + template; refuses overwrite; rejects invalid slug |
| `init` | `wiki/decisions/README.md` created; gitignore exception added idempotently; existing gitignore content preserved |
| End-to-end CLI | `phr-mcp wiki-drift --json` round-trips; `--suggest` emits valid rule JSON for uncovered |
| MCP tool | `get_wiki_drift` returns expected payload; mirror coverage of CLI |

## Commit plan (4 commits)

1. **`feat(wiki): add wiki module with Decision parsing`** — new
   `wiki.rs`, frontmatter parser, `Decision` type, unit tests. No
   CLI, no integration.
2. **`feat(wiki-drift): extractor with Jaccard + enforces shortcut`** —
   new `wiki_drift.rs`, `run`/`render_*`/`suggest_rule`, unit tests.
3. **`feat: phr-mcp wiki-drift + decision new commands + MCP tool`** —
   CLI subcommands, MCP tool, integration tests.
4. **`feat(init): scaffold .phronesis/wiki/decisions/ + gitignore
   exception; bump phronesis-mcp 0.9.0`** — `init.rs` changes,
   version bump, CLAUDE.md docs.

Commits 1–3 each compile and pass tests independently. Commit 4 is
the user-facing release.

## Rollout

After installing 0.9.0:

1. `phr-mcp init --hooks-only` (or full `phr-mcp init`) on this
   project to create `.phronesis/wiki/decisions/` + README + gitignore
   exception.
2. Repurpose existing `docs/specs/SPEC-*.md` as the seed corpus:
   either symlink them in or write decision pages that reference
   them. Manual; one-time. The SPECs themselves stay in `docs/specs/`
   as implementation plans.
3. Run `phr-mcp wiki-drift` to see what's covered vs uncovered.
4. (Future) Same for `~/Git/rulgamr`.

## Open questions

- **Tags-as-namespaces.** `tags: [vocabulary, prose]` is free-form
  today. Should certain tags trigger different matching behavior
  (e.g., `tags: [auditable]` opts a decision into stricter coverage
  thresholds)? Defer.
- **Inverse drift.** Rules without any decision linking back to them
  ("orphan rules") are detectable from the same corpus once `enforces:`
  is in use. Worth a follow-up subcommand
  (`phr-mcp orphan-rules`). Defer.
- **Wiki page validity check.** Should `init` (or a separate
  `wiki-lint` command) validate frontmatter completeness across all
  pages? Probably yes for a maintainability sweep; defer.
