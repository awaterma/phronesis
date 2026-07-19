# SPEC: Python pack expansion and Python governance roadmap

**Status:** draft  
**Authors:** Codex, Andrew Waterman  
**Date:** 2026-07-18  
**Target release:** phronesis-mcp 0.21.x for the base-pack improvement;
later MINOR releases for new opt-in packs  
**Affects:** `crates/phronesis-mcp/src/init.rs`,
`crates/phronesis-mcp/src/syntax/python.rs`,
`crates/phronesis-mcp/src/syntax/facts.rs`, audit predicate wiring,
pack documentation, catalogue generation, and Python integration tests

## Summary

Improve the existing `python` starter pack first, keeping it a conservative,
low-noise set of correctness rules that is safe to enable for ordinary Python
projects. Then evaluate more opinionated rules as separate opt-in packs:

- `python-security`
- `python-typing`
- `python-pytest`
- `python-architecture`

The source of candidate rules should primarily be the maintained Ruff rule
catalogue, with the official Python documentation, Python typing documentation,
and individual upstream linter documentation used to validate semantics and
rationale. *Architecture Patterns with Python* (Cosmic Python) should inform
architecture guidance. `python-patterns.guide` may inform explanatory text, but
must not be treated as a comprehensive or normative best-practices catalogue.

Phronesis should not become a second general-purpose Python linter. It should
select rules where its hook timing, project facts, diff awareness, cross-file
knowledge, consequence severity, and durable explanations provide value beyond
running Ruff, a type checker, or a security scanner.

## Motivation

The current Python pack is small and uneven:

- `warn-print-in-src` uses a substring condition and can match comments,
  strings, non-call identifiers, and non-Python files under `src/`.
- `enforce-no-bare-except` also uses a substring condition even though an
  accurate `python_bare_except` AST fact already exists.
- `audit-python-bare-except` duplicates the pre-check rule with different
  detection semantics.
- `python_mutable_default_arg` is structurally useful but narrower than the
  established families of problematic default-value rules.
- `python_function_param_count_high` is extracted but no packaged rule consumes
  it.
- Missing docstrings and parameter-count limits are policy choices, not
  universal correctness requirements.

The pack needs a clearer contract: the base pack prevents or explains likely
defects; optional packs express project policy.

## Goals

1. Replace avoidable substring matching in the Python pack with tree-sitter
   predicates.
2. Expand the base pack with a small number of stable, low-false-positive
   correctness checks.
3. Give every shipped rule a traceable upstream source and rationale.
4. Keep rules configurable through existing rule-file mechanisms and preserve
   current phase semantics (`pre`, `post`, `audit`, and `none`).
5. Define which proposed rules fit the existing Rust implementation and which
   need new analysis capability.
6. Establish criteria for placing a rule in the base pack versus an opt-in
   pack.
7. Prefer Phronesis-native rules where project, diff, test, or tool history is
   required; leave commodity linting to established Python tools.

## Non-goals

- Reimplementing all Ruff, Pylint, Bandit, mypy, Pyright, or pytest lint rules.
- Performing whole-program Python type inference.
- Performing general data-flow or security taint analysis in the first release.
- Mandating one application architecture for all Python projects.
- Treating every Gang-of-Four pattern as a rule.
- Enabling opinionated documentation, typing, testing, or layering rules by
  default.
- Changing the rules-file wire format solely for this work.

## Source policy

### Primary candidate catalogue: Ruff

Use Ruff's stable rule catalogue to discover candidates and borrow established
identifiers in specification metadata. Ruff consolidates rule families from
Pyflakes, pycodestyle, pyupgrade, flake8-bugbear, flake8-bandit, flake8-pytest-
style, flake8-async, flake8-annotations, tryceratops, and others.

An upstream identifier such as `B008` or `B904` is provenance, not a promise of
exact behavioral parity. A Phronesis rule must document deviations caused by
tree-sitter analysis or project-aware semantics.

Do not copy upstream messages mechanically. Phronesis messages should explain:

1. the code shape detected;
2. the likely failure mode;
3. a practical correction; and
4. relevant project context when available.

### Normative rationale

Use these sources in descending order of authority:

1. Python language and standard-library documentation;
2. Python typing documentation and accepted PEPs;
3. the original upstream rule documentation and implementation tests;
4. mature framework documentation where the rule is framework-specific.

### Architecture sources

Use *Architecture Patterns with Python* for optional guidance concerning domain
models, repositories, service layers, units of work, ports and adapters,
dependency inversion, and event-driven systems. These are contextual patterns,
not universal syntax requirements, and therefore belong in an opt-in
`python-architecture` pack or project-defined rules.

Use `python-patterns.guide` only as a secondary explanatory source for subjects
such as composition over inheritance, decorators, adapters, iterators, and
flyweights. Its incomplete and intermittently maintained catalogue is not a
suitable source of truth for the base pack.

### Source record required for every rule

Each implemented rule must have a nearby code comment or catalogue metadata
record containing:

- upstream family and identifier, when applicable;
- authoritative reference URL;
- why Phronesis implements it instead of relying only on the upstream tool;
- known false-positive cases;
- default pack, phase, severity, and audit participation; and
- whether behavior is exact, narrower, or broader than upstream.

A future schema may expose this as first-class rule metadata. This specification
does not require a schema change; comments and generated catalogue data are
acceptable initially.

## Pack design principles

### Base `python` pack

The base pack must be safe for broad use. A rule belongs here only if:

- it detects a likely runtime defect or clearly misleading behavior;
- detection is syntax-driven and reliable without project-specific inference;
- common legitimate uses can be excluded cheaply; and
- the message can recommend a generally valid remedy.

Base-pack `block` actions require especially high confidence. New rules should
normally begin as `warn` or `audit`, gather field evidence, and be promoted only
after false-positive review.

### Optional packs

A separate pack is appropriate when a rule depends on threat model, typing
policy, test framework, project layout, architectural boundaries, or team
preference. Optional packs remain starter rule sets: after initialization their
rules live in `.phronesis/rules.json` and can be edited or removed normally.

Pack names should compose with the existing CLI:

```bash
phr-mcp init --packs llm,python
phr-mcp init --packs llm,python,python-security,python-pytest
```

Aliases may be considered (`py-security`, for example), but documentation and
serialization should use canonical `python-*` names.

## Phase 1: improve the existing Python pack

### 1. Replace substring bare-except enforcement

Change `enforce-no-bare-except` to consume `python_bare_except(?file, ?fn)` and
restrict it to Python files through the syntax fact itself or an explicit
extension condition. Remove the duplicate audit-only rule, or retain one rule
whose phase and audit behavior provide both hook and audit coverage.

Recommended default: `block`, because a bare handler also catches
`KeyboardInterrupt` and `SystemExit` and the AST match is precise.

Migration note: rule IDs should remain stable where practical so refreshed rule
files do not produce avoidable churn.

### 2. Replace substring `print()` detection

Add a `python_print_call(?file, ?fn)` predicate that recognizes a call whose
callee is the bare name `print`.

The first implementation should not flag:

- comments or string literals containing `print(`;
- methods such as `printer.print()`;
- identifiers such as `sprint()`;
- non-Python files;
- tests, examples, migrations, and CLI entry points when excluded by configured
  path conditions.

Recommended default: `warn`, not `block`. Printing is legitimate in CLIs and
scripts. The starter rule should target conventional application-source paths
and explain how projects can adjust path conditions. The official logging HOWTO
supports recommending a module-level `logging.getLogger(__name__)` for
application logging, but the message must not imply that all `print()` calls are
wrong.

### 3. Expand default-argument checks

Retain the existing mutable literal/container predicate, corresponding broadly
to Bugbear `B006`. Extend coverage deliberately rather than treating every call
as mutable.

Add a separate `python_call_in_default_arg(?file, ?fn, ?param, ?callee)` fact,
corresponding broadly to `B008`. Initially exclude known immutable constructor
calls only when they can be identified without name-resolution ambiguity.

Recommended defaults:

- mutable literal or `list`/`dict`/`set` constructor: `warn`, audit enabled;
- other calls in defaults: `audit` initially, promoted to `warn` after fixture
  and field validation.

Keep the predicates separate so projects can choose the stricter policy without
changing the existing rule.

### 4. Detect swallowed exceptions

Add structural facts for handlers whose body has no observable handling action:

- `python_exception_handler_passes(?file, ?fn, ?exception)` for a body that is
  only `pass`, comments, or an ellipsis;
- later, `python_exception_handler_swallows(?file, ?fn, ?exception, ?shape)` for
  simple constant returns or equivalent shapes.

Recommended default: warn only for typed handlers; bare handlers are already
blocked. Constant-return cases remain audit-only because fallback behavior may
be intentional.

Do not claim that logging and continuing is always correct. Recommend handling,
re-raising, or documenting the intentional fallback.

### 5. Preserve traceback semantics

Add precise predicates for:

- `raise e` (or the caught binding name) inside its handler, where bare `raise`
  preserves the original traceback;
- raising a new exception inside a handler without explicit chaining, broadly
  corresponding to `B904`.

Recommended default: `warn`, audit enabled. The extractor must associate a
`raise` with the nearest enclosing exception handler and must not flag unrelated
raises in nested functions.

**Phase 1 deferral:** Traceback-preservation rules remain deferred. Initial
tree-sitter fixtures showed that reliable detection needs an explicit scoped
handler walk: raises may occur in nested control-flow blocks, while raises in
nested functions and lambdas must be excluded, and caught bindings must be
associated with the nearest handler. A direct-child scan missed valid cases and
could not distinguish these scopes reliably. Ship these rules only after
positive, nested-control-flow, nested-function, lambda, and chained-exception
fixtures establish the required semantics.

### 6. Activate the existing parameter-count fact conservatively

Ship a rule consuming `python_function_param_count_high`, but keep it
`audit`-only. Parameter count is a maintainability signal, not a correctness
failure. Continue excluding `self` and `cls`; document how positional-only,
keyword-only, `*args`, and `**kwargs` contribute to the count.

The existing extractor must be tested against all modern Python parameter
forms before the rule is exposed.

### 7. Keep missing docstrings audit-only

The existing public-function docstring rule remains audit-only. Before expanding
it, define "public" more accurately for methods, nested functions, overloads,
properties, protocol methods, and modules whose public surface is controlled by
`__all__`.

Do not promote missing docstrings to a base-pack hook warning in this phase.

## Candidate optional packs

### `python-security`

Candidate rules:

| Candidate | Initial action | Feasibility |
|---|---:|---|
| `eval()` / `exec()` calls | block or warn | supported by new call predicates |
| `subprocess` call with literal `shell=True` | warn | supported with attribute/call/keyword AST matching |
| `requests` call with literal `verify=False` | block | syntax supported; import alias resolution desirable |
| unsafe `yaml.load()` loader usage | block | syntax supported; import alias resolution desirable |
| weak hash used for security | audit | requires context; syntax alone is insufficient |
| unsafe pickle of untrusted data | audit | requires data-flow/taint analysis; defer |

Security rules must not overstate guarantees. Recognizing `requests.get` by
spelling is useful but is not proof of library identity. Pack documentation must
state whether aliases and re-exports are resolved.

### `python-typing`

Candidate rules:

- public API explicitly annotated with `Any`;
- `# type: ignore` without a specific error code;
- public parameters using concrete mutable containers where an abstract input
  protocol would be more appropriate;
- a package claiming typed distribution support while exposing incomplete
  public annotations;
- obsolete `typing.List`, `typing.Dict`, `typing.Optional`, and related syntax
  when the configured minimum Python version permits modern syntax.

Most of these begin as `audit`. Phronesis must read project configuration before
making version-dependent recommendations. It must not require type annotations
for all Python projects: Python remains dynamically typed, and official typing
guidance treats static typing as optional.

The type-completeness rule is cross-file and requires package/public-surface
analysis; it is not supported by the current per-file extractor alone.

### `python-pytest`

Candidate rules:

- `try`/`except` used where `pytest.raises` more clearly expresses the test;
- `pytest.raises(Exception)` or no match/message check in high-risk tests;
- test functions with no assertion, exception assertion, mock assertion, or
  other recognized verification;
- unconditional `pytest.skip` or `xfail` introduced without a reason;
- assertion removal from a changed test;
- production function added or materially changed without a corresponding test;
- changed subject with no relevant pytest invocation before a completion claim.

This pack is a strong fit for Phronesis because it can combine syntax, diff
facts, `test_exists_for`, and toolchain outcome signals. Test-verification
recognition must account for pytest idioms rather than searching only for the
`assert` keyword.

### `python-architecture`

This pack must be project-configured rather than pretending there is one
universal Python directory layout. Candidate policies include:

- domain modules may not import framework, ORM, HTTP, filesystem, or environment
  adapters;
- handlers/controllers delegate business behavior to an application or service
  layer;
- infrastructure dependencies enter through configured ports or protocols;
- side effects occur at configured boundaries;
- domain code remains independently testable.

Implementation requires a project-owned mapping of path groups and allowed
dependency directions, for example:

```json
{
  "python_architecture": {
    "layers": {
      "domain": ["src/*/domain/**"],
      "application": ["src/*/application/**"],
      "infrastructure": ["src/*/infrastructure/**"]
    },
    "forbid_imports": {
      "domain": ["django", "sqlalchemy", "requests"]
    }
  }
}
```

The exact configuration format is future work and may belong in a generic
architecture-policy feature rather than a Python-only pack. Until configuration
exists, ship documentation or example rules, not active generic constraints.

## Phronesis enhancements

### Required for Phase 1

1. Add the new Python fact collections to `SyntaxFacts` and its exhaustive
   drift-guard tests.
2. Add tree-sitter extractors in `syntax/python.rs`, continuing to parse each
   file once and run multiple extractors over the same tree.
3. Emit stable fact predicates and arguments through `SyntaxFacts::all_facts`.
4. Add all new predicates to the central predicate registry so hooks and audit
   recognize them consistently.
5. Ensure audit can evaluate every new per-file AST predicate.
6. Add starter rules and pack documentation.
7. Add catalogue provenance and regenerate the catalogue if pack rules change.

Source ranges or line numbers should be added to new facts if the common syntax
fact representation can support them without a wire-format break. Function-only
context is often ambiguous when a function contains multiple violations.

### Recommended shared enhancements

#### Import and callee normalization

Add a lightweight, file-local import table capable of resolving common forms:

```python
import requests as r
from subprocess import run as execute
```

This is not whole-program name resolution. It should resolve direct imports and
aliases only, record ambiguity, and avoid firing identity-sensitive rules when a
name is shadowed locally.

This enhancement materially improves security, async, typing-modernization, and
architecture rules.

#### Project Python configuration facts

Read relevant, bounded configuration from `pyproject.toml`:

- minimum supported Python version;
- Ruff target version and selected/ignored families;
- pytest configuration and test paths;
- type-checker strictness or typed-package intent;
- application versus test/example/script path conventions.

Configuration facts prevent recommendations that conflict with a project's
supported Python version or existing policy. Phronesis should honor explicit
project configuration over starter-pack defaults.

#### Rule provenance and overlap reporting

Eventually expose source identifiers and overlap in rule metadata or a
`check-rules`/pack-preview command. If Ruff already enforces `B008`, Phronesis
should be able to explain why its version remains enabled or recommend avoiding
duplicate hook noise.

#### External-tool result ingestion

The existing outcome/toolchain system recognizes pytest commands. Extend the
generic toolchain mechanism, rather than Python syntax analysis, if Phronesis
needs grounded results from Ruff, ty, mypy, Pyright, Bandit, or pytest.

Phronesis should consume tool outcomes for governance and completion confidence;
it should not parse and duplicate every external tool's internal rule logic.

### Deferred analysis capabilities

The following are not provided by the current implementation and should not be
promised by the first release:

- whole-program symbol resolution;
- control-flow graphs;
- interprocedural data flow;
- taint tracking from external input to dangerous sinks;
- inferred types;
- framework-aware call semantics;
- automatic discovery of architectural layers without configuration.

## Current Rust implementation capability assessment

The current Rust implementation is sufficient for most of Phase 1, but not for
the complete roadmap.

| Capability | Current support | Notes |
|---|---|---|
| Parse Python once with tree-sitter | yes | `ParsedFile::parse_python` and `syntax/python.rs` |
| Emit multiple AST facts per file | yes | `SyntaxFacts` and WME conversion |
| Bind file/function/value arguments in rules | yes | existing Python facts demonstrate this |
| Hook-time block/warn and audit-only behavior | yes | existing phases and consequences |
| Add new starter-pack variants | yes | extend `StarterPack` and `init.rs` |
| Combine AST and path/extension facts | yes | normal RETE multi-condition rules |
| Diff-aware function/import/test facts | partial | existing generic facts; Python fidelity varies |
| Pytest command/outcome recognition | partial | toolchain matcher exists; subject relevance needs work |
| Direct call and keyword-argument matching | requires extractor work | tree-sitter supports the syntax; predicates do not yet exist |
| Direct import-alias resolution | no | recommended lightweight enhancement |
| Cross-file public API/type completeness | no | requires project index |
| Security taint analysis | no | defer or rely on dedicated scanners |
| Configurable architecture graph | no | requires a new generic policy/configuration surface |

Therefore, Phase 1 does not require changing the RETE engine. It primarily
requires extending the Python syntax analyzer, fact registry, starter rules,
and tests. Some optional packs can begin with syntax-only checks, while their
more ambitious rules require shared MCP analysis features rather than core RETE
changes.

## Rule-selection workflow

For every proposed upstream rule:

1. Classify it as correctness, security, typing, testing, architecture, style,
   or modernization.
2. Determine whether Ruff or another standard tool already handles it fully.
3. State the additional value Phronesis provides.
4. Measure whether tree-sitter syntax is sufficient.
5. Enumerate legitimate code shapes and exclusions.
6. Choose `block`, `warn`, `audit`, or reject the rule.
7. Add positive, negative, nesting, aliasing, and malformed-source fixtures.
8. Run the rule against representative real projects before promotion.

Reject candidates that are purely formatting preferences, require unavailable
semantic analysis, or cannot achieve an acceptably low false-positive rate.

## Compatibility and rollout

### Existing initialized projects

Starter packs are copied into project rules files. Changing `python_rules()`
does not silently update existing projects. Documentation must show the existing
refresh flow:

```bash
phr-mcp init --rules-only --force --packs python
```

Because `--force` can replace customized starter rules, provide a dry-run or
clear diff-oriented instructions if available. Do not imply that refresh is
lossless.

### Pack naming and semver

Improving rule precision without changing intended behavior can be PATCH-level.
Adding user-visible rules, new predicates, or new pack names is a pre-1.0 MINOR
change under the repository's release policy. Phase 1 should therefore target a
MINOR release if it adds rules, even if individual fixes could ship earlier.

### Severity promotion

New rules follow this maturity path unless the issue is unambiguously dangerous:

```text
audit -> warn -> block
```

Promotion requires fixture coverage, audit results from representative projects,
and no unresolved common false-positive class.

## Testing requirements

### Unit tests

Each extractor needs tests covering:

- the exact positive syntax shape;
- near-miss identifiers, attributes, strings, and comments;
- nested functions, lambdas, classes, decorators, and async functions;
- malformed/incomplete source that tree-sitter can still parse;
- multiple violations in one function;
- modern parameter syntax;
- import aliases where relevant; and
- suppression/exemption behavior where supported.

### Pack tests

Verify:

- canonical and alias pack parsing;
- generated rule IDs, phases, priorities, audit flags, and messages;
- pack composition without duplicate IDs;
- refreshed rules JSON validity; and
- catalogue output when rules change.

### Hook and audit integration

For every new AST predicate, include at least one integration test proving it
fires through the hook path and one proving it participates in audit when
configured to do so. Test both violation and clean fixtures.

### Quality gate

```bash
cargo fmt --all
cargo test --workspace
cargo clippy --workspace -- -D warnings
```

## Acceptance criteria for Phase 1

- [ ] Bare-except enforcement uses the Python AST and no longer relies on
      `new_content_contains`.
- [ ] `print()` detection is Python-call-aware and excludes comments, strings,
      methods, and non-Python files.
- [ ] Mutable and called default arguments have documented, separately
      configurable predicates.
- [ ] At least one swallowed-exception predicate is available and covered by
      hook and audit integration tests.
- [ ] Traceback-preservation/chaining candidates are implemented or explicitly
      deferred with fixture evidence explaining why.
- [ ] The existing high-parameter-count fact has an audit-only packaged rule and
      correct modern-parameter tests.
- [ ] Missing docstrings remain audit-only.
- [ ] Every new rule records upstream provenance, deviations, default severity,
      and known limitations.
- [ ] Documentation distinguishes Phronesis governance from Ruff linting and
      recommends complementary use.
- [ ] Existing project refresh behavior and customization risk are documented.
- [ ] Workspace format, tests, and clippy pass.

## Follow-up decision points

After Phase 1 ships and field data is available:

1. Decide whether `python-security` or `python-pytest` provides the highest-value
   next pack. `python-pytest` is likely the best demonstration of Phronesis's
   unique cross-file and tool-history strengths; `python-security` has the
   clearest high-severity syntax checks.
2. Decide whether lightweight import normalization should be a shared syntax
   service before either pack expands.
3. Decide whether rule provenance warrants a rule-schema extension.
4. Decide whether architecture boundaries belong in a language-neutral pack
   driven by project configuration.
5. Decide whether tool-result ingestion for Ruff and type checkers should
   precede additional native lint predicates.

## References

- Ruff rule catalogue: <https://docs.astral.sh/ruff/rules/>
- Ruff configuration: <https://docs.astral.sh/ruff/configuration/>
- Python documentation: <https://docs.python.org/3/>
- Python logging HOWTO: <https://docs.python.org/3/howto/logging.html>
- Python typing best practices:
  <https://typing.python.org/en/latest/reference/best_practices.html>
- Typing Python libraries:
  <https://typing.python.org/en/latest/guides/libraries.html>
- Architecture Patterns with Python: <https://www.cosmicpython.com/book/>
- Python Design Patterns: <https://python-patterns.guide/>
