#!/bin/sh
# Regenerates export.jsonl from REAL per-test cargo-llvm-cov runs.
#
# Per-test attribution by isolation (SPEC-coverage-evidence §2/§8): each
# test runs alone under coverage, so every executed region in that run is
# attributable to that test.
#
# Requirements: cargo-llvm-cov 0.8.x, a nightly toolchain (--branch
# instrumentation), python3. Run from this directory: ./regenerate-export.sh
set -eu
cd "$(dirname "$0")"

REV=$(git rev-parse HEAD)

for t in divides_positive_values divides_negative_values rejects_zero_denominator; do
  cargo +nightly llvm-cov --json --branch -- "$t" > "/tmp/cov-$t.json"
done

python3 - "$REV" <<'PYEOF'
import json, pathlib, sys

REV = sys.argv[1]
SRC_PATH = pathlib.Path("src/lib.rs")
LINES = SRC_PATH.read_text().splitlines()
TESTS = ["divides_positive_values", "divides_negative_values", "rejects_zero_denominator"]

def fnv1a12(text):
    # Must match region_map::compute_anchor: FNV-1a 64, first 12 hex chars.
    h = 0xcbf29ce484222325
    for b in text.encode():
        h ^= b
        h = (h * 0x100000001b3) % (1 << 64)
    return format(h, "016x")[:12]

def prod_name(mangled):
    # v0 mangling: last segment after the crate name, strip the length prefix.
    seg = mangled.split("coverage_sample")[-1]
    return seg.lstrip("0123456789")

def condition_text(line_no):
    line = LINES[line_no - 1].strip()
    cond = line[line.index("if ") + 3:]
    return cond[: cond.rindex(" {")].strip()

records = []
for t in TESTS:
    data = json.load(open(f"/tmp/cov-{t}.json"))["data"][0]
    for f in data.get("functions", []):
        if f.get("count", 0) == 0:
            continue
        if "5testss_" in f["name"]:
            continue  # test-module functions are not production regions
        name = prod_name(f["name"])
        if not name:
            continue
        spans = f.get("regions") or []
        start = spans[0][0] if spans else 1
        end = spans[-1][2] if spans else len(LINES)
        # Region ids follow SPEC-coverage-evidence §3.2 (file-qualified). The
        # fixture has only free functions and one `if` per condition, so the
        # item path is the bare name and no ordinal is ever needed.
        records.append({
            "v": 1, "kind": "hit", "test": t, "region": f"fn:src/lib.rs::{name}",
            "file": "src/lib.rs", "start_line": start, "end_line": end,
            "hit_kind": "region", "revision": REV, "tool": "cargo-llvm-cov",
        })
        # Branch hits: executed branch regions of this function.
        for br in f.get("branches", []):
            line, col, eline, ecol, count = br[0], br[1], br[2], br[3], br[4]
            if count == 0:
                continue
            anchor = fnv1a12(condition_text(line))
            records.append({
                "v": 1, "kind": "hit", "test": t,
                "region": f"branch:src/lib.rs::{name}:{anchor}",
                "file": "src/lib.rs", "start_line": line, "end_line": eline,
                "hit_kind": "branch", "revision": REV, "tool": "cargo-llvm-cov",
            })

with open("export.jsonl", "w") as fh:
    for r in records:
        fh.write(json.dumps(r) + "\n")
print(f"wrote {len(records)} records at {REV}")
PYEOF