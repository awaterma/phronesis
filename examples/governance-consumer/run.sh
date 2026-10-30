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
# Then exercises the noncoding operator flow over an expense-approval
# policy (a non-software domain, not a coding rule in sight):
#   6. build the operator_flow example, then run it from an EMPTY cwd with
#      explicit absolute fixture paths and assert verdicts and exit codes:
#      approve (exit 0), warn (exit 1), deny (exit 2), --phase audit
#      selecting the audit:true rule (Selection::Audit, not Hook(Audit)),
#      and a rejected extension spoof of a reserved predicate that no host
#      fact currently carries (exit 3)
#   7. print result proof, clean up only the temp dirs this script created
#
# The cargo target directory and both crate versions are derived from
# `cargo metadata --offline --no-deps`, so the script works with
# CARGO_TARGET_DIR unset, absolute, or relative. Never publishes; offline
# validation only.
#
# Usage: examples/governance-consumer/run.sh
# Exit 0 = PASS, nonzero = FAIL (the temp dir is kept, with every evidence
# log, whenever the script exits nonzero).

set -eu

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
stamp=$(date +%Y%m%dT%H%M%S)-$$
base_tmp="${TMPDIR:-/tmp}/governance-consumer-$stamp"
package_tmp="$base_tmp/package"
consumer_tmp="$base_tmp/consumer"

say() { printf '%s\n' "$*"; }
fail() {
    printf 'FAIL: %s\n' "$*" >&2
    exit 1
}

cleanup() {
    rc=$?
    if [ "$rc" -ne 0 ]; then
        say "kept for inspection (exit $rc): $base_tmp"
        return
    fi
    rm -rf "$base_tmp"
}
trap cleanup EXIT

mkdir -p "$package_tmp" "$consumer_tmp/src"
say "== 1. packaging governance + phronesis (--no-verify, offline) =="

repo_root=$(CDPATH= cd -- "$script_dir/../.." && pwd)
cd "$repo_root"

# Derive the cargo target directory and the two crate versions from cargo
# metadata itself: CARGO_TARGET_DIR may be unset (cargo default), absolute,
# or relative to the repo root, and the versions move with the workspace.
metadata_json=$(cargo metadata --offline --no-deps --format-version 1 \
    2>"$package_tmp/metadata.log") \
    || fail "cargo metadata; see $package_tmp/metadata.log"

target_dir=$(printf '%s\n' "$metadata_json" \
    | grep -o '"target_directory":"[^"]*"' | head -n 1 \
    | sed 's/.*"target_directory":"//; s/"$//')
[ -n "$target_dir" ] || fail "no target_directory in cargo metadata; see $package_tmp/metadata.log"

phronesis_version=$(printf '%s\n' "$metadata_json" \
    | grep -o '"name":"phronesis","version":"[^"]*"' | head -n 1 \
    | sed 's/.*"version":"//; s/"$//')
governance_version=$(printf '%s\n' "$metadata_json" \
    | grep -o '"name":"phronesis-governance","version":"[^"]*"' | head -n 1 \
    | sed 's/.*"version":"//; s/"$//')
[ -n "$phronesis_version" ] || fail "no phronesis version in cargo metadata; see $package_tmp/metadata.log"
[ -n "$governance_version" ] || fail "no phronesis-governance version in cargo metadata; see $package_tmp/metadata.log"
say "derived target dir: $target_dir"
say "derived versions: phronesis $phronesis_version, phronesis-governance $governance_version"

phronesis_crate_name="phronesis-$phronesis_version.crate"
governance_crate_name="phronesis-governance-$governance_version.crate"

cargo package -p phronesis --no-verify --offline --allow-dirty \
    >"$package_tmp/phronesis-package.log" 2>&1 \
    || fail "cargo package -p phronesis; see $package_tmp/phronesis-package.log"
cargo package -p phronesis-governance --no-verify --offline --allow-dirty \
    --config 'patch.crates-io.phronesis.path="crates/phronesis"' \
    >"$package_tmp/governance-package.log" 2>&1 \
    || fail "cargo package -p phronesis-governance; see $package_tmp/governance-package.log"

phronesis_crate="$target_dir/package/$phronesis_crate_name"
governance_crate="$target_dir/package/$governance_crate_name"
[ -f "$phronesis_crate" ] || fail "missing $phronesis_crate"
[ -f "$governance_crate" ] || fail "missing $governance_crate"

say "== 2. extracting crates into $base_tmp =="
tar -xzf "$phronesis_crate" -C "$base_tmp" \
    || fail "extracting $phronesis_crate"
tar -xzf "$governance_crate" -C "$base_tmp" \
    || fail "extracting $governance_crate"
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
phr = { package = "phronesis", version = "$phronesis_version" }
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

say "== 5. packaged-consumer result proof =="
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

say "== 6. expense-approval fixtures through the operator flow (empty cwd) =="
cd "$repo_root"
cargo build -p phronesis-governance --example operator_flow --offline \
    >"$base_tmp/operator-build.log" 2>&1 \
    || fail "building operator_flow example; see $base_tmp/operator-build.log"
operator_bin="$target_dir/debug/examples/operator_flow"
[ -f "$operator_bin" ] || fail "missing operator_flow binary at $operator_bin"

empty_cwd="$base_tmp/empty-cwd"
fixtures_dir="$script_dir/fixtures"
mkdir -p "$empty_cwd"
rules_fixture="$fixtures_dir/expense-rules.json"

run_operator() {
    # run_operator <name> <expected-exit> <events.json> [flags...]
    # Runs the operator flow from an empty cwd with explicit absolute
    # paths, captures output as evidence, and fails unless the exit code
    # matches exactly.
    name=$1
    expected_exit=$2
    events_file=$3
    shift 3
    op_log="$base_tmp/$name.log"
    op_rc=0
    (cd "$empty_cwd" && "$operator_bin" "$rules_fixture" "$events_file" "$@") \
        >"$op_log" 2>&1 || op_rc=$?
    [ "$op_rc" -eq "$expected_exit" ] \
        || fail "$name exited $op_rc, expected $expected_exit; see $op_log"
}

run_operator expense-approve 0 "$fixtures_dir/expense-events-approve.json"
grep -qF "verdict: Allow" "$base_tmp/expense-approve.log" \
    || fail "approve verdict marker missing; see $base_tmp/expense-approve.log"
grep -qF "reserving 4 host-owned predicate(s)" "$base_tmp/expense-approve.log" \
    || fail "explicit reserved_predicates not honored; see $base_tmp/expense-approve.log"
if grep -q "fired\[" "$base_tmp/expense-approve.log"; then
    fail "approve run fired a rule at hook phase pre; see $base_tmp/expense-approve.log"
fi

run_operator expense-warn 1 "$fixtures_dir/expense-events-warn.json"
grep -qF "verdict: Warn" "$base_tmp/expense-warn.log" \
    || fail "warn verdict marker missing; see $base_tmp/expense-warn.log"
grep -qF "receipt" "$base_tmp/expense-warn.log" \
    || fail "receipt warning message missing; see $base_tmp/expense-warn.log"

run_operator expense-deny 2 "$fixtures_dir/expense-events-deny.json"
grep -qF "verdict: Deny" "$base_tmp/expense-deny.log" \
    || fail "deny verdict marker missing; see $base_tmp/expense-deny.log"
grep -qF "approval limit" "$base_tmp/expense-deny.log" \
    || fail "limit block message missing; see $base_tmp/expense-deny.log"

# --phase audit must select audit:true rules regardless of their disk
# phase (Selection::Audit); a Hook(Audit) selection would match only
# disk-phase "audit" rules and would fire nothing here.
run_operator expense-audit 0 "$fixtures_dir/expense-events-approve.json" --phase audit
grep -qF "verdict: Allow" "$base_tmp/expense-audit.log" \
    || fail "audit verdict marker missing; see $base_tmp/expense-audit.log"
grep -qF "rule=expense-audit-trail" "$base_tmp/expense-audit.log" \
    || fail "audit selection did not fire the audit:true rule; see $base_tmp/expense-audit.log"

# The spoofed extension asserts rule_overridden, a predicate the events
# configuration reserves but which NO host fact currently carries, and it
# claims source "host" — reservations are configuration, not derivation,
# and source is never authorization. The snapshot must be rejected (exit 3).
run_operator expense-spoof 3 "$fixtures_dir/expense-events-spoof.json"
grep -qF "asserts reserved predicate \`rule_overridden\`" "$base_tmp/expense-spoof.log" \
    || fail "reserved-predicate spoof was not rejected; see $base_tmp/expense-spoof.log"

say "== 7. operator-flow result proof =="
for pair in \
    "expense-approve:exit: 0 (allow)" \
    "expense-warn:exit: 1 (warn, proceed)" \
    "expense-deny:exit: 2 (deny, block)" \
    "expense-audit:exit: 0 (allow)"; do
    name=${pair%%:*}
    marker=${pair#*:}
    grep -qF "$marker" "$base_tmp/$name.log" \
        || fail "$name exit-code marker missing: $marker"
done

say "PASS: standalone consumer exercised the packaged phronesis-governance crate offline"
say "PASS: expense-approval operator flow verified from an empty cwd (approve/warn/deny/audit/spoof)"