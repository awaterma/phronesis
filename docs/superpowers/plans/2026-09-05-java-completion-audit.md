# Java implementation completion audit

Status: **complete for the requested review, revision, and implementation**.
This checks revision 8 against local evidence and the completed Qwen review.
Documented analysis limitations remain; completion is not compiler-equivalent
resolution or a claim that the code has been released.

| Requirement | Current evidence | Remaining verification/work |
|---|---|---|
| Qwen 3.8 review and revision | Authorized review returned six proposed blockers; concrete fixtures verified dispositions, a malformed module-name defect was fixed, and reassessment returned PASS | Complete; see `REPORT-java-qwen-review.md` |
| Declared package identity, nested member types, import forms | Named tests below cover package identity, three-level nested types, local/anonymous exclusion, import forms, ambiguity, and source-set precedence; qualified record-pattern regression passes | Covered locally; Qwen reassessment PASS |
| Maven roots, parent chains, profiles, helpers, scope visibility | `java/maven/tests.rs` covers parent/profile/root order and helper merging; classic scope tests assert exact visibility sets; unsupported Maven 4 scopes and ignored modifiers are documented and counted | Covered within the spec's stated approximations; Qwen reassessment PASS |
| Bazel source ownership and target visibility | `java/bazel/tests.rs` covers globs, nested package boundaries, exports versus transitive deps, unknown macros, filegroups, aliases/select, unsupported expressions, and unclaimed sources; project test verifies target-specific import resolution | Covered locally; Qwen reassessment PASS |
| Content-validated cache | Process-local/disk round-trip and invalidation tests; real CLI same-size/same-mtime change plus corruption/deletion recovery; manifest overlay test recomputes units/classification and freshness; real MCP manifest changes work with the retained declaration cache; both corpora match pre-cache bytes | Covered locally; Qwen reassessment PASS |
| Canonical function/coverage identities | Static-call fixture, persistence target validation, and passing regressions for explicit `this`, lexical methods/types, import precedence, ambiguous receiver types, overloads, direct constructed receivers, and unresolved inherited methods; both corpora retain graph parity | Supported shapes verified locally; Qwen reassessment PASS. Ordinary variable receivers and inherited dispatch are documented unresolved shapes, not claimed coverage |
| Watched zero-argument API calls | Passing `watched_api_calls_require_a_receiver_zero_arguments_and_an_executed_body`: all five API names, nonzero-argument and receiverless exclusions, deferred/nested-body exclusions | Covered by direct regression; retain canonical identity checks |
| Save/delete/manifest invalidation | Eight Java sync tests; format 20; CLI manifest/rebuild parity; real MCP tests for both backends; real audit command reports exactly the two cycle files and excludes the clean file for both backends | Covered locally; Qwen reassessment PASS |
| Shipped cycle rule and file scoping | Real-binary Maven/Bazel cycle, clean-file scoping, and manifest-only refresh tests passed in the post-cache workspace run | Covered; package-level meaning retained |
| Four-language integration through the real binary | `java_manifest_hook_preserves_four_languages_and_matches_a_clean_binary_rebuild` passed in the post-cache workspace run: four language namespaces, canonical Java coverage, manifest-driven rename, unchanged other-language edges, exact clean-rebuild parity | Covered |
| Maven/Bazel corpus accounting | `REPORT-java-code-graph-corpus.md`: all skipped/unclaimed/mismatch groups partitioned, sampled cycles checked | Accounting verified and limitations retained; no compiler-equivalent resolution claim |
| Measured performance | CPU sampling motivated the disk cache; isolated release hooks now take about 2.2 s on Maven and 2.7 s on Bazel, preserving pre-cache graph bytes | Measured cost documented; no low-latency claim |
| Documentation and release handling | Public docs, release notes, grammar provenance/licenses; final package listing includes cache module, build script, grammar source, generated parser, and both licenses; cache lifecycle/Maven 4 limitations documented; historical checklist distinguished from current work | Covered locally; release-plz handles version changes; catalogue unchanged because packaged rules are unchanged |
| Final gates and review | Current workspace build, Clippy with tests/examples, formatting, scoped whitespace check, 2,487 Rust tests and 46 BDD scenarios pass, including final review regressions | Final fix reviewed, Qwen reassessment PASS, both corpus graphs unchanged; repeat relevant gates for future changes |

The corpus audit now accounts for Maven's 29 path mismatches as well as its
78 unresolved imports and seven parse failures (114 skipped items total).
All Bazel skipped/unclaimed groups are also accounted for. Accounting explains
why the current graph omits evidence; it does not waive the remaining rows.

## Test inventory checkpoint

The following names identify direct assertions rather than relying on a
passing suite count alone:

- `declarations_follow_packages_and_member_paths_not_files_or_local_classes`
  covers declared package/member identities and local-class exclusion.
- `parses_each_import_form_including_java_25_module_import` and
  `exact_import_forms_resolve_without_ancestor_fallback` cover syntax and
  declaration-index resolution separately.
- `test_context_shadows_main_without_hiding_production_types`,
  `split_package_selection_is_local_then_unique_visible_owner`, and
  `build_visibility_filters_same_unit_and_cross_unit_candidates` cover owner
  selection, source-set precedence, and visibility.
- `only_method_annotations_identify_tests_and_nested_bodies_do_not_supply_calls`
  checks the four test annotations and excludes helper, lambda, local-class,
  and anonymous-body evidence.
- `repository_roots_do_not_share_same_named_source_state` passed for two
  independent roots and deletion of one root's cached input.
- `watched_api_calls_require_a_receiver_zero_arguments_and_an_executed_body`
  passed for all five watched names and excluded call shapes.
- `compile_visibility_propagates_scopes_and_excludes_unrelated_units`
  asserts exact production/test dependency sets for compile, provided,
  runtime, and test scope propagation, including unrelated-unit exclusion.
- `helper_execution_ids_merge_before_materializing_child_roots` and
  `helper_configuration_append_override_and_plugin_inheritance_are_distinct`
  cover Maven helper merging before root derivation.
- `dependencies_are_target_specific_and_follow_exports_but_not_transitive_deps`
  asserts different classpaths for two Bazel targets in one BUILD file and
  follows exports without leaking a dependency's implementation dependencies.
- `unknown_macros_claim_sources_but_entry_points_and_runtime_deps_do_not`
  checks a literal `srcs` custom macro and verifies that a source-free
  `java_test` does not turn its runtime dependency into test source.
- `nested_build_boundaries_and_filegroups_preserve_test_classification`
  and `globs_selects_bindings_and_test_claims_are_order_independent` cover
  recursive glob boundaries, filegroup expansion, and conflicting claims.

This is a partial inventory. Maven/Bazel evaluator cases, host consumers,
final gates, and independent review still require the evidence above.

Workspace build passed before the disk-cache experiment. The first workspace test run stopped in three
catalogue tests because the CLI executable was absent during a concurrent
build. The serialized full rerun passed (2,478 Rust tests and 46 BDD scenarios),
including the cache-root and watched-API regressions. The subsequent disk-cache
experiment adds serialization and atomic cache persistence; 757 graph tests,
the CLI regression, and isolated performance repeats pass. The current full
workspace test run passed 2,483 Rust tests and 46 BDD scenarios. Subsequent
serialized workspace Clippy/build and formatting checks also passed.

The follow-up `java_maven_and_bazel_graphs_refresh_through_mcp_tools` passed
through the real stdio server: it rebuilds both backends, queries Java cycle
results, detects manifest staleness, and verifies a Bazel visibility split
removes cycles while a Maven artifact rename updates returned module IDs.
This adds direct MCP evidence beyond the shared sync implementation and CLI
tests. Workspace Clippy and formatting pass after this test-only addition.

Direct real-binary `audit --rule warn-import-cycle --json` runs on fresh
Maven and Bazel fixtures each reported exactly two warned files (`a/A.java`
and `b/B.java`), zero blocked findings, and no finding for an unrelated clean
package. This verifies the audit consumer as well as the hook and MCP paths.

## Final review closure

Qwen reviewed the authorized bundle and returned six proposed blockers. Local
fixtures disproved its Bazel claims and confirmed the documented Maven
coordinate behavior. A refined module-import fixture exposed a real reserved
word acceptance defect, fixed with regression coverage and cache format 2.
Qwen reassessed the evidence and returned PASS. The final workspace tests,
Clippy, build, formatting, whitespace check, and both corpus graph comparisons
passed. The detailed review report distinguishes reviewer inferences from
executed evidence. No outstanding implementation or review action remains for
this objective; publication and future scope extensions were not requested.
