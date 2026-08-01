# SPEC: TypeScript code-graph extractor

**Status:** design, approved 2026-07-31
**Target release:** 0.24.0 (MINOR — new extractor, new pack rules)
**Parent spec:** `docs/specs/SPEC-triple-store-rete.md` (rev 6)
**Affects:** `crates/phronesis-mcp/src/graph/{unit,typescript,sync,mod}.rs`,
`crates/phronesis-mcp/src/init.rs` (pack rules), `docs/catalogue.html`

## Summary

A third extractor for the structural code graph, covering TypeScript. Unlike
Python, TypeScript can support **both** shipped structural rules, because the
non-null assertion `!` is a defensible watchlist entry where Python had no
equivalent.

The hard part is not parsing. It is **import resolution**: Rust and Python
imports name modules, so an edge falls out of the source text, but TypeScript
imports name *paths* (`./billing`, `@app/billing`) that must be resolved
against the filesystem and `tsconfig.json` before an edge can be drawn. That
resolution is the feature, and it is where this design spends its care.

## Why resolution is the risk

A missing import edge is invisible. It does not error, and it does not look
different from a codebase that genuinely has no such dependency — it looks
like a clean result. `imports` feeds `in_cycle`, so every dropped edge is a
cycle the pack silently fails to report.

This already happened once: Python's cross-distribution imports were dropped
for exactly this reason and were only found by explicitly testing for them.
TypeScript has far more opportunities to drop edges, because resolution is
doing real work on every import rather than none.

**Consequence for the design:** an unresolved *relative* import is counted in
`skipped`, never silently discarded. A specifier beginning with `.` names
something inside the project by definition, so failing to resolve it is a bug
in this extractor, not third-party code.

## Identity

`ts:<package>::<module path>`, with no target suffix — npm has no analogue of
Cargo's library/binary/test target split, the same reasoning that gave Python
no suffix.

The package name comes from `package.json`'s `name` field. A `.ts` file no
`package.json` claims falls back to `ts:project`, matching Python's
`python:project`.

Module paths derive from the file path relative to `baseUrl` (or the unit root
when `baseUrl` is unset), extension stripped, a trailing `/index` stripped,
segments joined with `::`:

```
src/billing/charge.ts   →  ts:myapp::billing::charge      (baseUrl "./src")
src/billing/index.ts    →  ts:myapp::billing
lib/util.ts             →  ts:myapp::lib::util            (no baseUrl)
```

This **must** be a pure function of the path. Resolution computes an identity
from two directions — the importing file's specifier and the target file's own
path — and an edge only forms when they agree.

Segments join with `::` in every language; see
`.phronesis/wiki/decisions/2026-07-27-graph-identity-separator.md`.

## Discovery

`UnitMap::discover` gains TypeScript alongside Cargo and Python:

- Every `package.json` in the tree defines a unit. **Many units per repo is
  the normal case** — `frontend/` and `server/` each with their own manifest
  is not a monorepo, just two projects, and the innermost-unit rule already
  handles it.
- Each unit reads its own `tsconfig.json`: `compilerOptions.baseUrl`,
  `compilerOptions.paths`, and `extends` chains. Each unit's config is its
  own; two units in one tree do not share resolution rules.
- Each unit indexes its `.ts`, `.tsx`, `.mts`, `.cts` files, so the extractor
  can resolve by lookup rather than by I/O.

**`node_modules` is excluded unconditionally**, in discovery and in the file
index — not via `.gitignore`. Every dependency ships a `package.json`, so a
naive walk mints hundreds of units and indexes tens of thousands of files,
making the per-save cost unusable. This is the same principle already applied
elsewhere (third-party crates are not Rust units; third-party imports are not
Python edges); npm merely requires an explicit filter because it puts
dependencies physically inside the tree.

Build output (`dist/`, `.next/`, `coverage/`) is left to `.gitignore`, which
covers it in practice. A project that commits `dist/` is a real case to handle
when encountered, not to guess at now.

## Resolution — pre-resolved in discovery

Three approaches were considered:

- **A. Filesystem access inside the extractor.** Rejected: it makes the same
  file yield different edges depending on disk state at that instant, and puts
  I/O on the per-save hot path once per import.
- **B. Emit unresolved specifiers, resolve in `derive`.** Architecturally
  defensible — resolution *is* whole-graph work — but introduces a base
  relation that exists only as an intermediate, weakening the closed relation
  set as a promise to rule authors.
- **C. Pre-resolve in discovery.** **Chosen.** Discovery already walks the
  tree and reads manifests; it is the layer that already owns disk access.
  `UnitContext` carries the resolution inputs forward and the extractor stays
  pure.

Resolution order per specifier, mirroring `tsc`:

1. **Relative** (`./x`, `../x`) — resolve against the importing file's
   directory, probing `.ts`, `.tsx`, `.d.ts`, `.js`, `.jsx`, then `/index.*`.
2. **Non-relative** — try `paths` aliases, then `baseUrl`.
3. **Otherwise** — third-party. No edge, no count. A node with no definitions
   hanging off it is worse than no node.

Unresolved relative imports increment `skipped` (see "Why resolution is the
risk").

## Relations

`file_type`, `declares_module`, `defines_fn`, `imports`, `calls_api`,
`tested_by` — the existing closed set, no additions.

**Test files:** `*.test.ts`, `*.spec.ts` (and `.tsx` variants), or any file
under a `__tests__/` directory.

**`tested_by` needs a different shape.** In Rust and Python, tests are named
functions (`#[test] fn foo`, `def test_foo`). In TypeScript they are callbacks
passed to `it()` / `test()`, usually anonymous:

```ts
it("charges the order", () => { placeOrder(cart) })
```

The coverage source is therefore identified by its **title string**:
`ts:myapp::billing.test::charges the order`. Only calls inside `it` / `test`
callbacks count — not calls in helpers elsewhere in the file. This mirrors
Python's rule that only `test_*` functions are evidence: a helper's calls are
not proof anything was verified.

**`calls_api` watchlist: the non-null assertion `!`**, emitted as
`calls_api(fn, "non_null_assertion")`.

This is weaker than Rust's watchlist and the difference is recorded here
deliberately. `.unwrap()` panics *at the call site*; `!` erases at compile
time and surfaces as a `TypeError` later, possibly far away. The rule
therefore claims "this function makes an unchecked assumption and nothing
tests it", not "this function can panic and nothing tests it". That is a
useful claim, and a different one.

## Rules shipped

Both, matching the Rust pack's structure and `warn`-only per the pack's
audit → warn → block maturity policy:

| id | fires on |
|---|---|
| `warn-untested-risky-call` | production function using `!` with no direct test |
| `warn-import-cycle` | module in an import cycle |

Both join `edited_file` so they report the file in front of the user rather
than the whole repository on every edit.

## Out of scope

- **Monorepos and cross-unit imports.** An import resolving to another unit in
  the tree (`@yourorg/shared`) produces no edge in v1 — but it is **counted in
  `skipped`**, not dropped silently. This is precisely the gap that shipped
  unnoticed in Python; shipping it visible is the correction.
- **Project references** (`references` in tsconfig).
- **`.js` / `.jsx` extraction.** Resolution will find them, but they get no
  `declares_module` edge, so such imports are dangling and counted.
- **Promotion to `block`.** Awaits a measured corpus, as for Rust and Python.

## Testing

Unit tests per concern, following the Python extractor's structure: identity,
`tsconfig` parsing (including `extends` chains and a missing tsconfig),
resolution (each branch of the order above, plus the unresolved case
incrementing `skipped`), `tested_by` title extraction, `!` detection.

Integration tests through the real binary, extending
`tests/graph_structural_rules.rs`: a TS project producing a graph, a cycle
detected, both rules firing, and a mixed Rust/Python/TypeScript repository
proving three languages coexist.

**Validation against a real project is required before merge, not optional.**
phronesis contains no TypeScript, so there is no in-repo corpus. Synthetic
fixtures will pass while resolution quietly drops edges on real code — the
failure mode this whole design is organised around. The extractor must be run
against an actual TypeScript project and its edge counts compared against
what a human knows is there.

## Risks

1. **Resolution silently under-reports.** Mitigated by counting unresolved
   relative imports and by the real-project validation gate above.
2. **Discovery cost.** `UnitMap::discover` runs per save; TypeScript adds a
   file index on top of the manifest walk. Current per-save is 6.5–10 ms.
   Measure rather than assume; cache by directory mtime if it bites.
3. **`!` proves too noisy.** It is common in some codebases. If the corpus
   shows a poor precision rate, demote that rule to `audit` before promoting
   anything.
