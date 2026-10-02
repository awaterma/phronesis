# Swift lcov fixture

No SwiftPM project is committed here, and no `.build/` artefacts are
committed. The lcov file and manifest are hand-crafted in the
`llvm-cov export -format=lcov` record format that `swift test
--enable-code-coverage` produces (live-probed on Swift 6.4): `TN:` carries
the graph's Swift test id (`swift:<unit>::<file segment>::<class>::<name>`),
`# node:` the runner-native `StoreTests.StoreTests/testLoad` filter name,
`FN:`/`FNDA:` carry mangled symbol names (`$s…` — never matched for
attribution, since Swift sets `one_liner_needs_fnda: false`), and the `DA:`
counts carry the executed body lines. `Store::load` is a multi-line body hit
(`DA:3,1` inside `[2,4]`); `Store::oneLiner` is a one-liner and stays
`unattributable` even though its body line shows a positive count — it is
never attributed from the declaration line. The integration test replaces
the manifest revision and digest after copying the fixture into a temporary
git repository.

To regenerate with SwiftPM + llvm-cov when available, run per-test isolated
collection and export:

    swift test --enable-code-coverage --filter '^StoreTests\.StoreTests/testLoad$'
    cp "$(swift test --show-codecov-path | xargs dirname)/default.profdata" /tmp/testLoad.profdata
    xcrun llvm-cov export -format=lcov -instr-profile /tmp/testLoad.profdata \
        "$(swift build --show-bin-path)/StoreTests.xctest/Contents/MacOS/StoreTests" \
        > cov/swift__StoreTests__StoreTests__testLoad.lcov

then prepend `TN:`/`# node:` and refresh `manifest.json`.