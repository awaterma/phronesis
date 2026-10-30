#!/bin/sh
# Acceptance script for the standalone out-of-workspace governance consumer.
#
# Exercises the PACKAGED governance crate, not a path into workspace source:
#   1. cargo package --no-verify the governance and phronesis crates
#   2. extract both .crate files into a fresh temp dir
#   3. assemble a standalone consumer (own [workspace]) that depends on the
#      extracted governance crate and patches its phronesis dependency to the
#      extracted phronesis crate
#   4. build and run the consumer fully offline
#   5. print result proof, clean up only the temp dirs this script created
#
# Never publishes; offline validation only.
#
# Usage: examples/governance-consumer/run.sh
# Exit 0 = PASS, nonzero = FAIL (failed temp dirs are kept for inspection).

set -eu

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
stamp=$(date +%Y%m%dT%H%M%S)-$$
base_tmp="${TMPDIR:-/tmp}/governance-consumer-$stamp"
package_tmp="$base_tmp/package"
consumer_tmp="$base_tmp/consumer"
keep_on_fail=0

say() { printf '%s\n' "$*"; }
fail() {
    keep_on_fail=1
    printf 'FAIL: %s\n' "$*" >&2
    exit 1
}

cleanup() {
    if [ "$keep_on_fail" = 1 ]; then
        say "kept for inspection: $base_tmp"
        return
    fi
    rm -rf "$base_tmp"
}
trap cleanup EXIT

mkdir -p "$package_tmp" "$consumer_tmp/src"
say "== 1. packaging governance + phronesis (--no-verify, offline) =="

repo_root=$(CDPATH= cd -- "$script_dir/../.." && pwd)
cd "$repo_root"

phronesis_crate_name=phronesis-0.40.0.crate
governance_crate_name=phronesis-governance-0.40.0.crate

cargo package -p phronesis --no-verify --offline --allow-dirty \
    >"$package_tmp/phronesis-package.log" 2>&1 \
    || fail "cargo package -p phronesis; see $package_tmp/phronesis-package.log"
cargo package -p phronesis-governance --no-verify --offline --allow-dirty \
    --config 'patch.crates-io.phronesis.path="crates/phronesis"' \
    >"$package_tmp/governance-package.log" 2>&1 \
    || fail "cargo package -p phronesis-governance; see $package_tmp/governance-package.log"

phronesis_crate="$CARGO_TARGET_DIR/package/$phronesis_crate_name"
governance_crate="$CARGO_TARGET_DIR/package/$governance_crate_name"
[ -f "$phronesis_crate" ] || fail "missing $phronesis_crate"
[ -f "$governance_crate" ] || fail "missing $governance_crate"

say "== 2. extracting crates into $base_tmp =="
tar -xzf "$phronesis_crate" -C "$base_tmp"
tar -xzf "$governance_crate" -C "$base_tmp"
phronesis_dir="${phronesis_crate_name%.crate}"
governance_dir="${governance_crate_name%.crate}"
[ -f "$base_tmp/$governance_dir/Cargo.toml" ] || fail "governance crate did not extract"

say "== 3. assembling standalone consumer (own workspace, patched phronesis) =="
cp "$script_dir/consumer/src/main.rs" "$consumer_tmp/src/main.rs"
cat >"$consumer_tmp/Cargo.toml" <<EOF
[workspace]

[package]
name = "governance-consumer"
version = "0.1.0"
edition = "2021"

[dependencies]
phronesis-governance = { path = "$base_tmp/$governance_dir" }
phr = { package = "phronesis", version = "0.40.0" }
tokio = { version = "1", features = ["macros", "rt"] }

[patch.crates-io]
phronesis = { path = "$base_tmp/$phronesis_dir" }
EOF

say "== 4. building + running the packaged consumer offline =="
cd "$consumer_tmp"
CARGO_TARGET_DIR="$base_tmp/target" cargo build --offline \
    >"$base_tmp/build.log" 2>&1 \
    || fail "offline build of packaged consumer; see $base_tmp/build.log"
CARGO_TARGET_DIR="$base_tmp/target" cargo run --offline --quiet \
    >"$base_tmp/run.log" 2>&1 \
    || fail "offline run of packaged consumer; see $base_tmp/run.log"

say "== 5. result proof =="
cat "$base_tmp/run.log"
for marker in \
    "deny verdict: Deny" \
    "allow verdict: Allow" \
    "warn verdict: Warn" \
    "demoted verdict: Warn" \
    "isolated verdict: Allow" \
    "all packaged-crate checks passed"; do
    grep -qF "$marker" "$base_tmp/run.log" || fail "proof marker missing: $marker"
done

say "PASS: standalone consumer exercised the packaged phronesis-governance crate offline"