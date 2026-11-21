# Documentation policy

How documentation in this repository is organized, updated, and retired. Agents maintaining docs should invoke the `maintain-phronesis-docs` skill (`.agents/skills/maintain-phronesis-docs/SKILL.md`); this page is the canonical policy the skill links to, not a duplicate.

## Canonical hierarchy

Eight purpose directories under `docs/`, each with a `README.md` index reachable from [docs/README.md](../README.md):

| Directory | Holds | Does not hold |
|---|---|---|
| `docs/tutorials/` | First successful setup and end-to-end examples | How-to for a single task (Guides) |
| `docs/guides/` | Task-oriented instructions | Factual surface descriptions (Reference) |
| `docs/reference/` | CLI, MCP, configuration, predicates, language support | Explanations of behavior (Architecture) |
| `docs/architecture/` | Explanations of implemented behavior | Designs with open questions (Specs) |
| `docs/decisions/` | ADRs (stable IDs, preserved schema) | Spec proposals |
| `docs/specs/` | Proposed or evolving designs with explicit status | Point-in-time records (Archive) |
| `docs/contributing/` | Development, testing, releases, doc maintenance | User-facing how-tos (Guides) |
| `docs/archive/` | Historical plans, reviews, reports, superseded designs | Anything still maintained |

Rules:

- Every retained document has exactly one category, and every active document is reachable from an index. One canonical document per active topic; update it before creating another.
- Crate READMEs, `CLAUDE.md`, fixtures, executable examples, and the pattern guides stay beside their code; indexes may link to them but they are not moved.
- The root `README.md` stays concise: overview, quick start, and pointers. `CHANGELOG.md` and `NOTICES.md` stay at the repository root.
- A handful of paths are pinned by shipped code (string assertions in Rust). Such a path may move only when a forwarding stub is retained at the original asserted path, so the shipped strings and tests keep resolving; the stub is marked `status: historical`, links to the canonical page, and is recorded in [`docs/migration-map.json`](../migration-map.json). Retiring a stub requires the code change that owns the new path.

## Spec lifecycle statuses

Specs carry a status blockquote at the top and a row in `docs/specs/README.md`. Allowed statuses: `shipped`, `partially implemented`, `needs evidence` (proposed), and companion records of a shipped parent. Status must be derived from code and test inspection — the header names the implementation and test files with clickable relative paths — and first-release claims only from Git tags (`git tag --contains` on the introducing commit); a matching `CHANGELOG.md` topic is not implementation evidence. Never invent a status; if verification is incomplete, say `partially implemented` or `needs evidence` and list exactly what is known and what was not found. When a spec ships, update its header, the index row, and the architecture/reference page that now describes the shipped behavior.

## Historical records

Plans, reviews, validation reports, and superseded designs move to `docs/archive/` intact: no rewording, no retroactive edits. Add a historical notice (blockquote or HTML comment) naming the original location, and a replacement link where a current document exists. The archive stays public in Git but outside website navigation; never delete or rewrite archived records, and never treat them as current guidance.

## Moves and the migration map

Every documentation move is recorded in [`docs/migration-map.json`](../migration-map.json) (`moves[].from`/`moves[].to`). The website toolchain consumes the map for redirects and forwarding documents; update it in the same change that moves files, and never reuse an old path for new content.

## Untracked and private material

Inventory tracked documents with `git ls-files` only. Untracked local drafts are never published, moved, indexed, or copied into Git by documentation work.

## Checks

Structural requirements (index reachability, internal links, spec status headers, migration-map consistency) are enforced by the shared documentation check command run in CI; run the same command locally before handing off documentation changes and report what passed, what remains uncertain, and any deferred work.


After the website/CI layer is present, use the shared gates:

```sh
python3 -m pip install -r scripts/checker-requirements.txt
python3 -m unittest discover -s scripts/tests
scripts/check-docs.sh --source-only --strict
scripts/check-catalogue.sh
scripts/website/build.sh
python3 scripts/website/gen_redirects.py --site docs/_site
scripts/check-docs.sh --site docs/_site --strict
```

The source checker inventories tracked Markdown. Stage an intentionally added
public document before checking it; private drafts stay untracked. The build
adds untracked and ignored documentation paths to its exclusions before Jekyll
reads page contents. Review desktop/mobile rendering and keyboard navigation
when changing templates, CSS, or navigation. A passing checker is structural
evidence; it does not establish that editorial claims are true.
