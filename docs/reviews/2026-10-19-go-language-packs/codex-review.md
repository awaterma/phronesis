**Not ready for a PR.** I reviewed `01513a7..503ec14`, including Tasks 4–8 and the four implementation fixes. The branch still permits host-predicate spoofing, unsupported test-pass evidence, and silent loss of failed extraction. The bundled Go pack also fails its own validator. No repository files were changed; the pre-existing untracked review directory remains unchanged.

**1. Findings — most severe first**

Paths below are relative to `crates/phronesis-mcp/src/` unless stated otherwise. Findings are confirmed by code inspection; runtime reproduction is identified explicitly.

1. **BLOCKER — `language_packs/facts.rs:32`: Custom-fact names can impersonate host graph predicates and other packs.**  
   A pack with ID `file` emitting local fact `adapter` produces the host-owned predicate `pack_file_adapter`. Three supplied arguments yield the same four-argument shape as the host’s adapter-provenance fact. Likewise, `a_b`/`c` and `a`/`b_c` both produce `pack_a_b_c`. These facts enter the rule network without reservation checks. **Fix:** reject host-reserved generated names and use an unambiguous pack namespace; add host and cross-pack collision tests.

2. **MAJOR — `outcomes/toolchain.rs:463`: Per-test counts can manufacture a passing test-run verdict after failure or truncation.**  
   Suppose `go test -v` prints one `--- PASS:` line, then another test panics before printing a verdict, and the process exits nonzero. Counts become `(1, 0)`; the nonzero-exit branch declares the build passed, and the emitted test fact becomes `outcome:test_pass`. An output prefix ending after that passing line also passes without an exit code or completion marker. **Fix:** separate individual observations from completed-run verdicts; a nonzero exit must veto an aggregate test pass, and incomplete output must remain inconclusive. The Verus model mirrors this behavior rather than excluding it.

3. **MAJOR — `language_packs/facts.rs:28`: Hook and audit extraction silently discard template diagnostics.**  
   A query can match while an emission references a missing or repeated capture. `extract` returns diagnostics and omitted records, but `facts_with_lines` consumes only `records`. A blocking fact can therefore disappear while pre-check allows and audit reports no incomplete evaluation. Graph extraction correctly rejects the same diagnostics. **Fix:** propagate extraction diagnostics as evaluation failures consistently across hooks, audit, and graph.

4. **MAJOR — `packs/go/declarations.scm:47`: Go shadow detection omits binding forms that can create false call and test relationships.**  
   Given package function `Work`, a test containing `for _, Work := range callbacks { Work() }` resolves the local callback invocation to the package function: range bindings are neither captured short declarations nor `var_spec` bindings. Function-literal parameters and receiver bindings have similar omissions. **Fix:** capture these bindings or conservatively leave affected references unresolved. The local-shadow test at `tests/go_language_pack.rs:271` contains no call expression, so that portion passes without exercising shadowed-call resolution.

5. **MAJOR — `language_packs/install.rs:299`: The installation lock follows unchecked symlinks outside the project.**  
   If `.phronesis/packs/.install.lock` is a dangling symlink to an outside filename, `.create(true).open(...)` creates that outside file. A FIFO target can also block opening the lock. Parent-component checks do not check this final component. **Fix:** open the lock without following links, require a regular file, and use descriptor-relative installation operations.

6. **MAJOR — `language_packs/extract.rs:392`: Failed emissions bypass the output limit and can exhaust hook memory.**  
   A manifest with 300 templates referencing a missing capture, applied to 8,000 identifier matches, accumulates 2.4 million diagnostics while emitting zero records. The record cap never applies, and this loop runs after bounded query execution. **Fix:** bound diagnostics and total emission attempts, or fail the extraction immediately on an invalid emission.

7. **MAJOR — `graph/sync/rebuild.rs:316`: A native incremental save can certify stale Go edges against new embedded assets.**  
   Build a graph using embedded pack digest A, then run a binary with digest B but the same graph format and save a Rust file through `on_save`. Only that native file is re-extracted, but all embedded digest entries are replaced with B. Freshness can now accept Go edges still produced by A. **Fix:** compare embedded inputs before incremental updates and rebuild affected evidence when they differ; never replace another producer’s freshness stamp without regenerating its edges.

8. **MAJOR — `audit/run.rs:74`: Unreadable pack sources can yield a falsely complete audit.**  
   A discovered pack-owned file containing invalid UTF-8 fails `read_to_string` and is silently skipped before pack error handling. Oversized excluded pack files are also skipped without marking the audit incomplete. **Fix:** record pack-owned read and limit failures in `incomplete`, using bounded source reads before allocation.

9. **MAJOR — `hook/mod.rs:220`: Broken installed packs cannot be repaired through the governed hook path.**  
   A damaged installed manifest blocks every relevant tool call, but its failure is attributed to `loader.json`. The repair exception accepts only the project rules or loader file, so editing the damaged manifest or running its removal is blocked; editing `loader.json` cannot repair it. **Fix:** provide a narrowly confined pack repair/removal path while continuing to evaluate unrelated rules. Add recovery tests that also reject arbitrary-source bypasses.

10. **MAJOR — `language_packs/graph.rs:40`: Declared capabilities do not control runtime execution and output.**  
    Graph rebuilding translates declarations from structural-facts-only packs, despite their lacking `graph`; fact extraction similarly lacks a structural-facts capability gate. A toolchains-only pack still sends owned sources through parsing, so unrelated syntax can block edits despite structural analysis never being declared. **Fix:** gate execution and translation by capability and validate contradictory record emissions.

11. **MAJOR — `audit/engine.rs:669`: Pack audit evaluates regex conditions as literal substrings.**  
    Combine a pack fact with `new_content_contains: "\\bdanger\\b"`. Hooks match source containing `danger`, while `scan_pack_rules` searches for the literal backslashes and reports no hit. Unsupported companion conditions also silently clear the match. **Fix:** share predicate evaluation with the hook path, and diagnose conditions audit cannot evaluate.

12. **MINOR — `crates/phronesis-mcp/packs/go/fixtures/basic.expected.json:1`: The shipped Go fixture expectation is incorrect.**  
    It expects empty records although the source contains imports, declarations, references, and a dot-import fact. **Reproduced:** `target/swarm/gate/debug/phr-mcp pack validate crates/phronesis-mcp/packs/go --json` exits **1**, reporting the mismatch. **Fix:** commit the actual reviewed expectation and add a gate validating every bundled pack’s fixtures.

13. **MINOR — `graph/sync/index.rs:173`: Freshness checking repeatedly reloads and recompiles the registry instead of sharing one snapshot.**  
    One freshness check loads directly, again through `walk`, again through `context_input_files`, and again through its `tracked_files` call. It also repeats whole-tree discovery, including projects without pack-owned sources. **Fix:** pass one validated registry and source population through the operation, then complete the specified warm latency comparison. The size of the latency regression is **(unconfirmed)**.

14. **MINOR — `language_packs/graph.rs:47`: Go test-file classification depends on finding a supported test declaration.**  
    A `helpers_test.go` containing only helpers becomes `file_type(..., "production")`, contradicting the requirement to classify `*_test.go` files as tests. **Fix:** represent file classification separately from discovered test declarations.

**2. Spec requirements checked and satisfied**

- Go uses embedded manifest/query/Rhai assets through the external-pack machinery.
- Manifest schema/API validation rejects unknown manifest fields and unsupported declarations.
- Declared Unix asset reads use bounded, descriptor-relative, symlink-refusing traversal.
- SHA-256 digests cover declared asset names and exact bytes; installation publishes the validated snapshot.
- Record validation checks source spans, reference owners, and command shape.
- Rhai disables dynamic `eval`, exposes immutable inputs, and enforces operation/depth limits.
- Graph identities preserve declaration sites and avoid resolving duplicate target sites.
- Go handles normalized receiver identities, aliased `testing` imports, nearest modules, and external test-package scopes.
- CLI and MCP inventory share discovery code; inspection does not execute adapters or fixtures.
- Pack rule layers retain project precedence and are excluded from project-rule autosave.
- Go static selection is distinguished from dynamic coverage; no Go collector row was added.
- Quiet output does not invent test counts; absent exit and absent textual evidence remain unknown.
- Compile-only evidence is accepted by the revised ingestion path.
- `files_indexed` excludes embedded-pack virtual freshness keys.
- Verification documentation explicitly limits Verus claims to modeled reimplementations.

**3. Verification and limits**

I ran **48 distinct tests from existing compiled executables**: 18 Go extraction tests, 15 Rhai adapter tests, seven template tests, three record-validation tests, and five per-test outcome tests. All passed. I also reproduced the failing bundled Go fixture validation above.

I did not rebuild those executables; their exact-HEAD provenance is **(unconfirmed)**. I did not independently reproduce the full workspace result, 23-step real-Go smoke, Verus run, MSRV matrix, or latency gate. I avoided workflows that create fixtures, logs, graphs, or artifacts under the repository. Test assertions received targeted review rather than an exhaustive assertion-by-assertion audit.

Windows confinement was not runtime-tested. Its asset-reader fallback at `language_packs/assets.rs:260` explicitly retains a check/open race; exploit reproduction is **(unconfirmed)**. I did not review the pre-existing untracked review directory as branch evidence.

