# Bundled Java grammar

Base: the published `tree-sitter-java` **0.23.5** crate from
https://github.com/tree-sitter/tree-sitter-java, under the included MIT license.
The generated C headers carry Tree-sitter's MIT license, included separately
as `LICENSE.tree-sitter` from the CLI's v0.25.10 tag.

Local change: `record_pattern` also accepts `scoped_type_identifier`.
Without it, valid qualified patterns such as `case Outer.Item(String value)`
and `value instanceof Outer.Item(String text)` fail to parse. This was
reproduced in 14 production files of the pinned Bazel corpus. Upstream master
inspected on 2026-09-05 retained the same restriction.

Generated using Tree-sitter CLI **0.25.10**, ABI **14**:

```sh
cd crates/phronesis-mcp/vendor/tree-sitter-java
tree-sitter generate --abi 14
```

`grammar.js` is the editable source. Commit regenerated `src/parser.c`,
`src/grammar.json`, `src/node-types.json`, and the Tree-sitter C headers.
The crate build script compiles this bundled C parser; it does not run Node,
download dependencies, or regenerate the parser. The Rust grammar wrapper
loads the generated language through `tree-sitter-language`.

When grammar or declaration extraction behavior changes, increment `FORMAT`
in `src/graph/java/project/cache/disk.rs` so previously persisted parses are
not reused under the new extractor.

Regression tests live in `src/graph/java/parse/tests.rs`. The corpus reporter's
`--parse-errors` mode checks this same bundled parser. Keep this narrow patch
until a published upstream grammar supports these cases; then replace the
bundle with that dependency and rerun Java, graph, and corpus validation.
