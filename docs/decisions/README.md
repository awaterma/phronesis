# Decisions

Architecture decision records (ADRs) with stable IDs, dates, and enforcement links. Each file is one decision whose first block is YAML frontmatter (`id`, `date`, `status`, optional `enforces`, `superseded_by`, `tags`); the body uses Context / Decision / Enforcement / Consequences sections. The existing schema, IDs, dates, and status vocabulary are preserved verbatim.

## Canonical location

Canonical ADRs live in this directory (`docs/decisions/`). The migration of the existing ADR corpus from the legacy location `.phronesis/wiki/decisions/` into `docs/decisions/` is performed by the configuration/ADR compatibility change (PR1) and lands here with unchanged filenames; both locations are read during the transition, and canonical documents take precedence for duplicate IDs.

## Index

- [Don't ship `.unwrap()` in src/](2026-05-30-no-unwrap-in-src.md)
- [Readable rule schema (when/then/predicate-as-key)](2026-05-30-readable-rule-schema.md)
- [Tag the pre-feature release before starting a feature branch](2026-05-30-tag-pre-feature-state.md)
- [Audit-only Rust idiom rules](2026-05-31-audit-rust-idioms.md)
- [Prefer borrowed slices in public API signatures](2026-05-31-borrow-ergonomic-apis.md)
- [Function and file complexity budgets](2026-05-31-complexity-budgets.md)
- [No .await on synchronous phronesis API methods](2026-05-31-no-await-on-sync-api.md)
- [Block LLM deflection disclaimers](2026-05-31-no-llm-deflection.md)
- [No panicking macros in production code](2026-05-31-no-panic-in-production.md)
- [Use typed errors, not Result<_, String>](2026-05-31-proper-error-types.md)
- [Rust code discipline rules](2026-05-31-rust-code-discipline.md)
- [Tests must have assertions or error propagation](2026-05-31-test-assertions-required.md)
- [Verify end-to-end before reporting done](2026-05-31-verify-before-done.md)
- [Always build and test with --workspace](2026-05-31-workspace-builds.md)
- [Audit functions with long outer-scope `let` ladders](2026-06-04-rust-let-binding-count-high.md)
- [Audit functions with multiple outer-scope `let mut` declarations](2026-06-04-rust-let-mut-count-high.md)
- [Audit `fatalError(` in Swift sources](2026-06-04-swift-fatal-error.md)
- [Warn on `as!` force-casts](2026-06-04-swift-force-cast.md)
- [Audit legacy C-style geometry constructors](2026-06-04-swift-legacy-constructor.md)
- [Audit legacy `arc4random` / `drand48` calls](2026-06-04-swift-legacy-random.md)
- [Audit `static var shared` mutable singletons](2026-06-04-swift-mutable-singleton.md)
- [Journey derivation scaling](2026-06-21-journey-derivation-scaling.md)
- [Undefined selector rejection](2026-06-23-undefined-selector-rejection.md)
- [Piped test output loses signal](2026-07-06-piped-test-output-loses-signal.md)
- [Release tag masks partial publish](2026-07-18-release-tag-masks-partial-publish.md)
- [Code-graph identities always separate segments with `::`](2026-07-27-graph-identity-separator.md)
- [Warn when the edited file's module participates in an import cycle](2026-07-29-import-cycle-detection.md)
- [Warn when production code adds a panic-capable call with no direct test](2026-07-29-untested-risky-call.md)

## Working with decisions

- Create: `phr-mcp decision new <slug>` scaffolds a page here.
- Drift: `phr-mcp wiki-drift` reports decisions without rule coverage.
- Decisions compared against rules during drift analysis live under both locations during the transition; see `AGENTS.md` for the discovery contract.
