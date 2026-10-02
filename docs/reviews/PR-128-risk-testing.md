# PR 128 risk-focused testing follow-up

This follow-up implements risk-focused testing rather than a blanket 100% coverage requirement. The local repair branch begins at `0665dbd`; the governed one-hour swarm is preserved at `/tmp/phronesis-swarm-pr128-risk-tests`. Nothing was pushed or merged.

Two reproduced collector defects were fixed. Sanitized report names could collide with a generated suffix (`a+b`, `a?b`, `a_b_2`), silently losing one test's attribution. The shared allocator now reserves every emitted name across Python, JavaScript, Swift and Lua and bounds filename length. A failed report read inside a shell AND-list could also be followed by manifest publication. Report existence/nonempty checks and standalone tagging/rename commands now stop collection before a completion manifest is written.

## Outcome-to-test matrix

| Important outcome | Durable regression | Required observation |
|---|---|---|
| Existing index with missing records | `coverage_store_integrity::missing_records_with_an_existing_index_are_corrupt_and_leave_both_gaps` | Exact corruption reason, no trusted index/hits/revision facts, both conservative gaps |
| Unsupported index format | `unsupported_index_format_is_corrupt_with_valid_records_and_digest` | A genuinely valid baseline becomes corrupt for the intended reason |
| Matching digest but wrong count | `matching_digest_with_wrong_record_count_is_corrupt_and_not_evidence` | Count mismatch cannot masquerade as fresh evidence |
| Record tool differs from index | `valid_records_with_a_different_index_tool_are_corrupt_and_not_evidence` | Exact tool mismatch, no evidence suppression |
| Oversized valid JSONL | `coverage_import::oversized_valid_jsonl_import_preserves_existing_records_and_index` | Size rejection preserves trusted records and index bytes |
| Malformed JSON or invalid record after a valid prefix | `malformed_json_after_a_valid_record_preserves_existing_records_and_index`, `malformed_record_after_a_valid_record_preserves_existing_records_and_index` | No partially committed replacement |
| Sanitized/native suffix collision and long IDs | `pytest::tests::report_stems_remain_unique_through_native_suffixes_and_truncation`; `coverage_report_names::collectors_preserve_every_tn_and_reject_late_failures` | Deterministic unique filenames, original TN identities survive all six runner variants |
| Partial success then runner failure or missing report | `coverage_report_names`; Node branch of `scripts/coverage-runner-smoke.py` | No final manifest; attempted rejected import leaves previous records/index unchanged |
| Real runner zero-match, actual failure, missing report | Node smoke's `node_collision_smoke` | Three distinct function/test hits first; all late failure outcomes preserve prior evidence |
| Invalid LCOV numeric fields | `coverage_risk_mapping::lcov_malformed_numeric_fields_reach_parser_and_preserve_verified_store_and_index` | Fifteen variants reach numeric parsing, reject and preserve old bytes |
| Invalid JaCoCo numeric fields | `jacoco_malformed_numeric_fields_reach_parser_and_preserve_verified_store_and_index` | Fifteen variants reach numeric parsing, reject and preserve old bytes |
| All source paths missing or ambiguous | `lcov_all_missing_or_ambiguous_sources_never_replace_prior_evidence`, `jacoco_all_missing_or_ambiguous_sources_never_replace_prior_evidence` | No first-match attribution or new records; previous store survives |
| Mixed resolved/missing/ambiguous sources | Both `*_mixed_resolved_missing_and_ambiguous_sources_import_only_grounded_hits` tests | Only one grounded exact new hit replaces a different prior identity; old hit absent |

Independent review found the original mixed-source success expectations matched the prior store and could pass a no-op importer. A fresh repair auction strengthened both tests to require different replacement identities and exact records. A separate reviewer reran the owning suite and approved the repair. Store and collector reviews found no remaining blockers. Reports, raw logs, bids, handoffs and all worktrees remain available in the swarm directory.

## Verification

At source commit `a248e4d`, the combined workspace passed **3,450 Rust checks across 103 harness summaries**, with zero failures and four default ignored cases, plus **55 BDD scenarios / 229 steps**. All three real Python/Swift ignored regressions were then executed explicitly and passed; the remaining ignored case is a documentation example. Workspace build, strict all-target Clippy, format/diff checks and blocking audit passed. Audit reported zero blocked and 259 advisory warnings; two binding-dependent script rules cannot run in the audit engine and were explicitly reported as skipped. Phronesis confidence is medium with grounded compile/tests signals; this is not proof.

A fresh instrumented coverage run followed by those three real regressions on unchanged source measured **4,873 / 5,156 lines (94.51%) across 18 `src/coverage/` files**. The earlier measurement was 93.62% over 5,142 lines; new allocator/tests change the denominator, so this is not a like-for-like percentage comparison. The JSON and raw logs are retained as `artifacts/combined-coverage.json`, `combined-coverage.log` and `combined-real-ignored.log`. Graph-selected collector testing identified 556 tests across 48 owning binaries; workers ran these binaries, and the root combined full suite repeats the owning coverage. The three toolchain-dependent ignored regressions are run explicitly. Two worker subprocess tests initially hit shared-target build races (ENOENT); their owning suites passed sequential reruns, with both failures and reruns retained. Final root Cargo checks run sequentially per target.

All nine real collection/import/changed-body-selection smoke modes passed on the combined collector implementation: Python, Swift, Vitest JavaScript, Vitest TypeScript, Jest, Node/c8, Maven/JaCoCo, Gradle/JaCoCo and Lua/Busted. Fixtures use paths with spaces and are preserved. Node additionally verifies the actual three-way collision and late failure preservation. Local runner versions differ from CI in places (Python 3.11 versus CI 3.12, Node 26 versus CI 22); Linux/macOS matrix validation still belongs to remote CI.

## Deliberate exclusions and interpretation

Aggregate LLVM line coverage is supporting evidence for Rust coverage machinery, including inline test helpers; it is not production-only or branch coverage and does not measure the generated Python, JavaScript, Swift, Lua or JVM programs. Real runner assertions supply evidence for those programs. No aggregate coverage report is imported as per-test evidence.

The remaining lower-value gaps do not justify chasing 100%: parser type-mismatch paths excluded by earlier typed validation, presentation-only/error-formatting branches, and platform-specific I/O failures that cannot be reliably reproduced here. Existing permission/lock tests cover meaningful store failures. This work does not establish every operating-system failure mode, adversarial schedule, branch, or downstream deployment. New important attribution, false-success or evidence-loss outcomes should gain regression tests when discovered.

The one-hour swarm used three inherited Codex profiles, eight auctioned contracts and independent implementer/reviewer assignments. Every auction recorded three live bids. Actual API token use and price were unavailable; zero ledger reservations are unpriced placeholders, not a claim of free execution. Cleanup was not performed, consistent with the user's preservation instruction. Remote PR readiness requires these local commits to be published and exact-head remote CI to pass; this report does not certify the unchanged remote PR head.
