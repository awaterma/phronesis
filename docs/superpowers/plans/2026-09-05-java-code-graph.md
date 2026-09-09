# Java code-graph implementation worklist

Design: [Java code-graph spec](../specs/2026-08-03-java-code-graph-design.md),
revision 8, implemented and reviewed. The full Maven and Bazel scope was retained.

Before the disk-cache optimization: workspace build, `cargo test --workspace --tests --examples`
(2,478 Rust tests and 46 BDD scenarios), workspace Clippy with tests/examples,
formatting, and scoped whitespace checks pass. Both corpora retain exact
cold/warm graph parity after call-resolution fixes. The new cross-process
declaration cache reduces isolated release hook time to about 2.2 seconds
on Maven and 2.7 seconds on Bazel, preserving pre-cache graph bytes. Its
757 graph tests and real-binary regression pass. Updated workspace build,
Clippy, formatting, 2,487 Rust tests, and 46 BDD scenarios pass after the
Qwen review fix. Qwen reassessment returned PASS; both corpus graphs remain
byte-identical to their pre-review baselines. The
[completion audit](2026-09-05-java-completion-audit.md) is the authoritative
remaining-work list; checkpoint paragraphs below record earlier evidence.

## Current implementation

The `graph/java/` module now includes Maven and Bazel discovery, source
parsing, declaration indexing, content-validated caching, and graph extraction.
Graph format 20 integrates Java sources and manifests with rebuild, save,
freshness, and the existing hook/MCP rebuild paths. The existing
`warn-import-cycle` rule covers Java package cycles; no duplicate rule is added.
Real-binary tests exercise Maven and Bazel cycle warnings, clean-file scoping,
and a BUILD-only edit through `post-check`.

At this checkpoint, 36 Java component tests and 57 graph-sync tests have passed.
The two new real-binary tests passed. A full `cargo test --workspace` passed
with local socket access (sandboxed metrics endpoint tests could not bind).
That full run preceded the final small Bazel-alias change, which has its own
passing regression test. Re-run final gates after remaining fixes. Public
corpus checkouts and report JSONs exist under `/private/tmp`; these are evidence,
not a passing corpus gate. Bazel still has substantial unresolved/unclaimed
inputs requiring reconciliation. See the corpus report when refreshed.

Follow-up containment checks now cover Java source and manifest overlays:
outside paths, parent traversal, hidden directories, `node_modules`, and
symlinks leaving the repository are excluded; absolute in-repository paths
normalize to canonical graph provenance, including not-yet-written sources.
Eight Java sync tests passed. A broader graph run exposed a concurrent cache
test assumption (a different root can evict an entry between discoveries).
Reuse is now checked with isolated cache entries, while the filesystem test
still checks equal-length, same-mtime content changes and deletion.
After these fixes, all 741 graph library tests passed, as did MCP library
Clippy with warnings denied, formatting, and scoped whitespace checks.

The subsequent Maven helper inheritance change passes all 743 graph library
tests, MCP library Clippy, and formatting. The Apache Maven corpus rerun
completed with cold/warm edge parity and unchanged graph/diagnostic output
relative to the previous corpus checkpoint. Full workspace gates still need
to run again after the remaining implementation and corpus work.

Conditional Bazel aliases pass all 744 graph library tests, MCP library
Clippy, and formatting. The Bazel corpus rerun retained cold/warm edge parity:
seven skipped statements are now evaluated and 68 external alias targets
are newly diagnosed; imports and cycles are unchanged. Of 884 unclaimed
files, 778 are reconciled as vendored ProGuard sources packaged into a ZIP,
without a Java compilation target. See the updated corpus report.

The corpus reporter now supports `--parse-errors`, independently using the
pinned Rust parser to emit ERROR/missing-node locations. All seven Maven
failures are generation templates or the deliberate `Bad.java` fixture.
The 14 Bazel failures expose qualified record patterns unsupported by the
grammar; a minimal simple-versus-qualified reproduction confirms the gap.
Upstream grammar has the same restriction, so a published dependency update
alone did not provide a fix. This is now resolved by a narrow bundled grammar
patch accepting scoped type identifiers in record patterns. Generated C and
licenses ship in the crate; normal builds do not regenerate the parser.
All 745 graph library tests pass, including valid/malformed qualified-pattern
regressions, and library/example Clippy passes. Pattern variable bindings
also participate in receiver-shadowing checks. Cargo's package listing
includes the generated parser, build script, source, headers, and licenses.
The Bazel rerun has zero parse failures, 377 recovered import edges, 720
recovered method definitions, and five recovered coverage edges. Independent
cold/warm and repeat runs retain identical graph and diagnostic output.

All 884 Bazel unclaimed files are now reconciled in a disjoint, BUILD-backed
partition (source archives, processor resources, ijar data/precompiled
fixtures, excluded adapters, temporary project fixtures, and five sources
absent from owning compilation declarations). All 798 counted Bazel package
path mismatches are also partitioned and explained by inspected source
declarations and non-conventional layouts. These accounting checks do not
close the separate import-resolution or grammar gates.

Bazel unresolved imports are now reconciled: 3,058 originate in unclaimed
source archives/resources/excluded or unmatched files with no compilation
classpath. The one claimed-source case exposed bare same-package labels;
supporting colonless dependencies/exports/aliases/filegroups resolves it.
The new corpus run has no unresolved imports in claimed sources, 3,856
skipped items, and 1,870 unresolved labels. Graph edges are unchanged because
the repaired import resolves to its own package. All 746 graph tests, library
and example Clippy, and formatting pass. External/generated imports and the
Maven corpus still require their own final coverage audit.

The Maven reporter now emits source ownership and candidates considered by
the actual visibility callback. All 202 unit collisions are in test fixture
trees; the sole parent cycle is explicitly intentional in MNG-11009. The 78
unresolved imports have no visible candidate and are partitioned in the
corpus report: external Plexus copies, relocated legacy APIs, fallback
fixture ownership, obsolete Maven API coordinates, and four unsupported
Maven 4 scope cases. This exposes the limits rather than equating a matching
type name with a compile dependency. The 29 Maven path mismatches and final
scope/coverage acceptance still need review.

Maven's 29 path mismatches are now fully partitioned: 15 duplicate-coordinate
fixture files, three beneath `pom` packaging, two without a local POM, three
outside configured/conventional roots, and six with actual package/path
differences. See [the completion audit](2026-09-05-java-completion-audit.md)
for the remaining requirements; corpus accounting does not imply completion.

The real-binary four-language gap is closed by
`java_manifest_hook_preserves_four_languages_and_matches_a_clean_binary_rebuild`.
It verifies Rust/Python/TypeScript/Java function identities, canonical Java
coverage, a Maven unit rename through `post-check`, unchanged edges for the
other languages, and exact equality with a fresh CLI rebuild. The test passes
(9.2 seconds), as do targeted integration-test Clippy and formatting.

Call-resolution review fixed false coverage from explicit `this` falling
through to static imports, ambiguous receiver types selected by method name,
and applicable overload groups discarded in favor of another candidate.
Lexical methods/types and explicit imports now take their proper precedence
over static/wildcard alternatives. Positive and negative regressions pass
within all 749 graph tests; library/example Clippy and formatting pass.
The Bazel corpus retains package/import/cycle/counter output, with 60 net
additional coverage edges from the corrected resolution. Instance receivers
and inherited-method interactions still need final review.

## Completion

The requested review, revision, and implementation are complete. Qwen's six
proposed blockers were checked against source, official semantics, and
concrete fixtures. A malformed module-name defect was fixed; Qwen reassessed
and returned PASS. The completion audit records final workspace and corpus
verification. Detailed dispositions are in `REPORT-java-qwen-review.md`.

## Original implementation checklist (historical)

This preserves the original scope. Consult the completion audit for current
evidence rather than treating every imperative below as unfinished work.

1. Obtain Qwen 3.8's independent review of the complete revised design and
   implementation. Sending the spec to the requested Spark endpoint was
   rejected by automatic approval review; explicit document-transfer approval
   is pending. Do not treat the existing request JSON as a running review or
   a reviewer response. Refresh its payload to the current spec when approved.
2. Review Maven inheritance semantics against the remaining corpus cases.
   Build-helper now preserves execution IDs, inherited plugin configuration,
   execution overrides, configuration append/override attributes, and
   inheritance flags before deriving roots. Regression fixtures cover these
   cases, including explicit execution opt-in beneath a non-inherited plugin.
   Reconcile remaining approximations in named diagnostics.
3. Review and extend the implemented restricted Bazel evaluator against the corpus: ordered variable bindings,
   literal lists, concatenation, `glob` with exclusions, `select` unions,
   unknown macros with `srcs`, filegroups, package boundaries, per-file
   target visibility, direct dependencies plus exports, production-wins
   classification, unclaimed files, and entry-point diagnostics.
   Conditional dependency aliases now union scalar-string `select` branches;
   traversal separates active recursion from already visited shared targets.
   Regression coverage includes both kinds of branch, true cycles, and
   rejection of mixed string/list selections.
4. Audit the documented mixed Maven/Bazel ownership precedence, source-root overlap,
   unclaimed-file fallback, and exclusion of multi-release source trees.
   Resolve the filegroup classification seam: a source container must not
   turn all test sources into production merely by being referenced.
5. Audit the implemented Java project index per extraction pass, with cached parsed
   declarations keyed by current content hashes. Cover equal-length,
   same-mtime edits, root isolation, removed files, and manifest changes.
6. Complete the extraction audit: package declarations, function identities,
   exact imports, watched zero-argument call shapes, and canonical
   `tested_by` targets. Resolve only evidence-backed unique callees; no
   repository-wide leaf-name guesses. Check overload ambiguity, local and
   anonymous types, deferred lambda bodies, default-package and Java 25
   compact-source handling, parse failures, and diagnostics.
7. Audit Java integration with discovery, source tracking, freshness, rebuild,
   save/delete paths, audit, and MCP. Build metadata must be freshness input
   without producing source edges. Source and manifest edits must refresh
   unchanged importers using the supplied edited content where applicable.
   Update the graph format so older persisted graphs rebuild.
8. Keep the existing language-neutral cycle rule; the real-binary tests prove Java reaches it. Read the participatory-governance
   guidance and structural-rule inventory before changing packaged rules.
   Verify live rule production and consumption through the real binary.
9. Run Maven and Bazel integration fixtures, package-cycle detection, and a
   mixed-language repository. Cover every numbered design test, not only the
   current isolated tests. Confirm manifest-only edits, declaration changes,
   ambiguity changes, deletions, parse failures, and incremental/rebuild parity.
10. Complete the corpus gate using the cloned Apache Maven and Bazel trees.
    Initial runs exposed non-UTF-8 inputs, ancestor source roots, and dependency
    aliases; these have regression fixes. Reconcile the remaining counts. Report units, modules, import edges,
    every named counter, skipped evidence, split packages, and cycles.
    Reconcile unexpected counts and manually confirm sampled cycles.
11. Update public documentation, regenerate the catalogue if pack rules change,
    add release notes, and use release-plz for release version changes.
    Preserve unrelated worktree changes.
12. Run workspace build/tests/Clippy and formatting, perform independent final
    diff review, and audit the complete design requirement by requirement.
    Qwen review and passing isolated tests do not substitute for the corpus
    and host-integration gates.
