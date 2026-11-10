# RETE propagation and Phronesis audit

Candidate work repairs issue #159 by propagating each new beta token through the
source-indexed queue. The queue carries the producing state ID; consumers receive
the token on the side declared by their input index. Inline child processing is
removed so terminal activations follow the same path at every depth.

## Evidence and scope

The regression suite inspects the agenda immediately after assertion, before a
full rescan can hide a missed incremental activation. It covers all six arrival
orders for three conditions and all 24 for four conditions, one/two-condition
controls, inconsistent bindings, multiple matches, retraction/reassertion, shared
joins and beta-produced right inputs. Initial red evidence had nine failed tests,
but one was an incomplete fan-out fixture; it was corrected before acceptance.
A failing permutation loop stops at its first failure, so the initial evidence
does not establish how many individual permutations failed.

Rules with identical condition sequences previously overwrote one shared terminal's
production marker. The candidate isolates claimed terminal joins per rule while
sharing intermediate joins and preserving the low-level setter's last-mark-wins
behavior. New terminals seed already-produced prefix tokens. Regression evidence
confirmed both missing per-rule activation and a longer rule failing to complete
when registered after its prefix had matched.

The strengthened beta suite passes all 25 tests. Its repair red run contained six
invalid-variable fixture failures and six behavioral failures; fixture failures
were corrected separately. The subsequent core run passed 186 tests, and the MCP
run passed 3,451 with four ignored. These runs preceded integration of the audit
patch and are not the final combined workspace evidence.

Rule removal/replacement produced four failures among six tests before the repair.
Clearing the removed rule's production markers, while retaining shared memories,
made all six pass; the subsequent core run passed 192 tests. The new cleanup helper
is crate-private. Long-running rule churn and retained join-state growth were not
benchmarked.

## Audit triage

Baseline: 741 files, zero blocked findings, 346 warnings across 14 rule categories.
Warnings are triage leads, not proof of defects. No blanket suppressions or broad
style refactors are included. The final candidate audit scanned 743 files and
reported zero blocked findings and 337 warnings across 12 categories. Unsafe-comment
findings changed from eight to zero, and string-concatenation findings from two to
zero. The new terminal-construction helper adds one binding-count advisory (nine
bindings), taking that category from 110 to 111; it remains visible.

| Rule category | Baseline warnings | Disposition |
| --- | ---: | --- |
| Unsafe without safety comment | 8 | Reproduced multiline-comment false positives and string-literal false negatives; repaired using comment nodes and bounded statement scope |
| String concatenation | 2 | Local rule metadata lacked shipped `doc_excepted`; narrow metadata repair verified two to zero findings |
| Allow dead code | 5 | Documentation/attribute association and intentionality need per-site review; deferred |
| If-let opportunity | 2 | Style advisory; no demonstrated behavioral defect |
| Manual error return | 22 | Style advisory across several error types; not evidence that every return uses String |
| Binding count | 110 | Complexity threshold; decomposition deferred |
| Mutable binding count | 91 | Complexity threshold; decomposition deferred |
| Parameter count | 41 | API/design advisory; refactoring deferred |
| Untested risky call | 17 | Direct-call graph coverage advisory, not proof of absent transitive coverage |
| File length | 16 | Decomposition advisory; existing exemptions retained; not all sites have verified plans |
| String IDs | 12 | Domain-type design advisory; no API migration attempted |
| Empty tests | 12 | Assertion-recognition/compile/panic-freedom review needed; `?` is already recognized |
| Import cycles | 6 | Intra-crate design advisory; no demonstrated runtime defect |
| Blocking async calls | 2 | One-shot metrics write path; no demonstrated runtime failure |

`init --rules-only --dry-run` preserved the customized string-concatenation rule;
it did not automatically heal its missing metadata. Only that rule's
`doc_excepted=true` was restored to the shipped definition; all 85 local rules
were retained. This gitignored worktree repair is not a shared-baseline change.

The audit skips two binding-dependent script guards. Graph reachability is
advisory and does not replace executed tests. Raw run artifacts stay outside the
public repository; private configuration and credentials are excluded.

## Verification and delivery

The audit repair passes 31 focused hazard tests, 41 syntax integration tests and
91 audit tests. A second red/green cycle caught three over-broad scope associations
in the initial repair (module, closure and outer-unsafe commentary); these were
candidate regressions, not established defects in the released baseline.
The focused unsafe audit changed from eight warnings to zero across 740 files.
Adjacent direct-body comments retain their existing semantics, including trailing
comments; comment-language interpretation remains heuristic.

A stale `values_integration.rs` entry in the project guide was corrected to the
existing, executed `syntax_integration.rs` target. Guidance/wiki drift reports are
triage leads: matching unwrap and typed-error rules exist despite low token-overlap
scores. Private auto-memory was not included in this repository audit.

The combined candidate passes 3,874 Rust tests with zero failures and four ignored,
plus all 55 Cucumber scenarios (229 steps across six features). Workspace build,
all-workspace formatting and Clippy with warnings denied pass. Source identity was
unchanged throughout the gate batch. The known-bug registry's seven named tests
were checked in the final log; successful results were ingested before marking
entries fixed. The candidate's grounded confidence band is high.

The first combined run stopped at two language-pack recovery test failures:
hand-written commands interpolated an executable path containing spaces without
shell quoting. The CLI-generated quoted repair command passed. A test-only repair
quotes executable/project words and exercises a spaced project root, preserving
production policy and negative assertions. All five focused recovery tests then
passed, followed by the full green batch. The failed run remains recorded.

Five probes using the freshly built CLI accept adjacent multiline/block safety
comments and reject string markers, unrelated module commentary and undocumented
unsafe blocks. The final graph is fresh; code drift reports no uncovered items.
Graph rebuild still reports 45,711 unresolved calls, 329 ambiguous calls and one
skipped item, so freshness is not complete resolution. Worktree governance checks
pass with all 85 local rules retained. Fleet results remain 99 pass, 20 fail and
18 warn; unrelated drift and runtime hook-approval uncertainty remain outside this
repair. Independent acceptance is recorded against the delivered commit in the
draft PR; this document records the executed validation.

Commands were serialized through the existing machine gate. Reproduction and
verification commands include:

```sh
cargo test -p phronesis --test beta_propagation --offline -j 4
cargo test -p phronesis --test beta_rule_replacement --offline -j 4
cargo test -p phronesis-mcp --test language_pack_repair --offline -j 4
cargo test -p phronesis-mcp --lib syntax::rust::hazards --offline -j 4
cargo test --workspace --offline -j 4
cargo build --workspace --offline -j 4
cargo fmt --all -- --check
cargo clippy --workspace --offline -j 4 -- -D warnings
```
No installed binaries replaced, services restarted, caches deleted, or unrelated
repositories changed. The installed swarmctl lacks `resolve --outcome`; the run
pins the compatible checkout binary. Other fleet drift is reported separately.
