#!/usr/bin/env python3
"""Executable check script for the first governed session tutorial.

Verifies the flow documented in docs/tutorials/first-governed-session.md
inside disposable temporary directories, using only the Python 3 standard
library. Each check gets its own fresh fixture directory.

    tdd_walkthrough        published red/block/green Rust exercise (requires Cargo)
    fixtures               example files and the deterministic marker rule
    missing_executable     an absent phr-mcp fails loudly, not silently
    setup                  the tutorial's init command wires hooks, MCP
                           registration, and preserves the shipped rules
    allowed_blocked_hooks  a plain edit passes the pre-check hook, an edit
                           containing the tutorial marker is blocked, and the
                           action log records why
    malformed_rules        syntactically broken and semantically invalid rule
                           files fail closed at load (exit 2)
    stale_config           a case-wrong phase and duplicate rule ids fail
                           closed too
    doctor_readonly        plain doctor and doctor --json create, modify, and
                           delete nothing in the governed project
    unobserved_trust       with no host configuration, doctor reports
                           host_hooks=fail, host_trust=not_observed, and
                           log_evidence=not_observed (exit 2)
    log_evidence_not_proof forged and stale log records never upgrade
                           host_trust or log_evidence past not_observed
    doctor_probe           --probe runs its disposable fixture rule and
                           matches both expectations without touching the
                           real project
    doctor_malformed_rules doctor on a malformed rules file reports a fail
                           finding and exit 2, with host_trust still
                           not_observed

Doctor checks require the installed phr-mcp binary to implement the doctor
subcommand. When it does not, those checks FAIL by default — a missing
required feature is never turned green; pass --allow-missing-doctor to
explicitly downgrade them to SKIP on a legacy binary.

Doctor JSON contract (agreed with the doctor builders, correlation
interface-v1): top-level {host, path, findings, probe}; findings entries
{id, status, message}; status in ok|warn|fail|not_observed; host_trust is
always not_observed; log_evidence stays not_observed even when a local log
exists (local evidence is not proof); probe is null unless --probe, then
{rule_id, cases, ok, note, error} with cases {label, expect_block,
exit_code, matched_expectation, rule_id, decision}. Message wording is
deliberately not asserted.

Usage:
    python3 check_tutorial.py [--phr-mcp PATH] [--allow-missing-doctor]
                              [--keep] [--list] [-v]

Exit code is 0 when every check passes or skips, 1 when any check fails.
"""

import argparse
import os
import shutil
import sys
import tempfile
from pathlib import Path

from tutorial_cases import CHECKS
from tutorial_support import FAILED, PASSED, SKIPPED, Result, build_project, run_command


def doctor_supported(phr_mcp, env):
    probe = run_command(
        [phr_mcp, "doctor", "--help"], Path(tempfile.gettempdir()), env=env
    )
    return probe.returncode == 0


def main(argv=None):
    parser = argparse.ArgumentParser(
        description="Verify the first governed session tutorial end to end."
    )
    parser.add_argument(
        "--phr-mcp",
        default=os.environ.get("PHR_MCP", "phr-mcp"),
        help="phr-mcp binary to use (default: PHR_MCP env or 'phr-mcp' on PATH)",
    )
    parser.add_argument(
        "--allow-missing-doctor",
        action="store_true",
        help="explicitly downgrade doctor checks to SKIP when the binary "
        "lacks the doctor subcommand (default: they FAIL)",
    )
    parser.add_argument(
        "--keep", action="store_true",
        help="keep the temporary directory and print its path",
    )
    parser.add_argument("--list", action="store_true", help="list checks and exit")
    parser.add_argument(
        "-v", "--verbose", action="store_true",
        help="print notes even for passing checks",
    )
    args = parser.parse_args(argv)

    if args.list:
        for name, _, tier in CHECKS:
            print(f"{name}\t{tier}")
        return 0

    results = {}
    notes_by_check = {}
    phr_mcp = args.phr_mcp
    binary_found = shutil.which(phr_mcp) is not None
    env = dict(os.environ)
    for key in list(env):
        if key.startswith("PHRONESIS_"):
            env.pop(key)

    tmp_root = Path(tempfile.mkdtemp(prefix="phronesis-first-governed-"))
    doctor_ok = False
    if binary_found:
        doctor_ok = doctor_supported(phr_mcp, env)

    for name, fn, tier in CHECKS:
        res = Result()
        runnable = True
        reason = ""
        if tier in ("binary", "doctor") and not binary_found:
            runnable = False
            reason = (
                f"'{phr_mcp}' not found on PATH; install it per tutorial "
                "Step 0 (cargo install --path crates/phronesis-mcp)"
            )
            res.fail(reason)
        elif tier == "doctor" and not doctor_ok:
            if args.allow_missing_doctor:
                runnable = False
                reason = (
                    f"'{phr_mcp}' does not implement doctor; released "
                    "binaries before the doctor feature lack it. Build from "
                    "a source checkout that has the feature for full "
                    "coverage (explicit legacy skip requested)."
                )
            else:
                res.fail(
                    f"'{phr_mcp}' does not implement doctor; released "
                    "binaries before the doctor feature lack it. Install a "
                    "source build per tutorial Step 0, or pass "
                    "--allow-missing-doctor to explicitly skip these checks."
                )
        if runnable:
            project = build_project(tmp_root, name)
            fn(phr_mcp, project, env, res)
        status = FAILED if res.failures else (PASSED if runnable else SKIPPED)
        results[name] = (status, res.failures)
        notes_by_check[name] = (res.notes, [reason] if reason else [])

    printed_path = False
    for name, _, _ in CHECKS:
        status, failures = results[name]
        notes, reasons = notes_by_check[name]
        print(f"{status:4}  {name}")
        for note in notes:
            if status != PASSED or args.verbose:
                print(f"       - {note}")
        for skip_reason in reasons:
            if status == SKIPPED:
                print(f"       skip reason: {skip_reason}")
        for failure in failures:
            print(f"       !! {failure}")
        if args.keep and not printed_path:
            print(f"       temp dir kept: {tmp_root}")
            printed_path = True

    if args.keep and not printed_path:
        print(f"temp dir kept: {tmp_root}")

    any_failed = any(s == FAILED for s, _ in results.values())
    passed = sum(1 for s, _ in results.values() if s == PASSED)
    skipped = sum(1 for s, _ in results.values() if s == SKIPPED)
    failed = sum(1 for s, _ in results.values() if s == FAILED)
    print(
        f"\n{passed} passed, {failed} failed, {skipped} skipped "
        f"(binary: {'yes' if binary_found else 'no'}, "
        f"doctor: {'yes' if doctor_ok else 'no'})"
    )
    if not args.keep:
        shutil.rmtree(tmp_root, ignore_errors=True)
    return 1 if any_failed else 0


if __name__ == "__main__":
    sys.exit(main())
