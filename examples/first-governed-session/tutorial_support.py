"""Fixture, command, and evidence helpers for the tutorial checks."""

import argparse
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

EXAMPLE_DIR = Path(__file__).resolve().parent
PRACTICE_DIR = EXAMPLE_DIR / "practice"

RULE_ID = "no-tutorial-blocked-marker"
BLOCK_SNIPPET = "TUTORIAL-BLOCKED-MARKER"
RULE_MESSAGE_FRAGMENT = "no-tutorial-blocked-marker"

DOCTOR_STATUSES = {"ok", "warn", "fail", "not_observed"}
TRUST_FINDING_ID = "host_trust"
LOG_EVIDENCE_ID = "log_evidence"
HOST_HOOKS_ID = "host_hooks"
PROBE_RULE_ID = "doctor-probe-no-unwrap"
PROBE_TEMP_PREFIX = "phr-mcp-doctor-probe-"

FAILED = "FAIL"
PASSED = "PASS"
SKIPPED = "SKIP"


class Result:
    def __init__(self):
        self.failures = []
        self.notes = []

    def fail(self, message):
        self.failures.append(message)

    def note(self, message):
        self.notes.append(message)


def run_command(argv, cwd, input_text=None, env=None):
    return subprocess.run(
        argv,
        cwd=str(cwd),
        input=input_text,
        text=True,
        capture_output=True,
        env=env,
    )


def hook_event(tool_name, file_path, new_content, old_content=""):
    tool_input = {"file_path": str(file_path)}
    if tool_name == "Write":
        tool_input["content"] = new_content
    else:
        tool_input["new_string"] = new_content
        tool_input["old_string"] = old_content
    return {"tool_name": tool_name, "tool_input": tool_input}


def build_project(parent, label):
    project = parent / f"practice-{label}"
    shutil.copytree(PRACTICE_DIR, project)
    return project


def govern_project(phr_mcp, project, env, result):
    init = run_command([phr_mcp, "init", "--hooks-only"], project, env=env)
    if init.returncode != 0:
        result.fail(
            f"phr-mcp init --hooks-only exited {init.returncode}: "
            f"{(init.stderr or init.stdout).strip()[:400]}"
        )
    return init.returncode == 0


def snapshot(root):
    state = {}
    for path in sorted(root.rglob("*")):
        rel = path.relative_to(root).as_posix()
        if path.is_file():
            state[rel] = hashlib.sha256(path.read_bytes()).hexdigest()
        else:
            state[rel + "/"] = "dir"
    return state


def changed_paths(before, after):
    changes = []
    for key in sorted(set(before) | set(after)):
        if before.get(key) != after.get(key):
            changes.append(key)
    return changes


def read_rules(path):
    return json.loads(path.read_text(encoding="utf-8"))


def load_log_entries(project):
    log = project / ".phronesis" / "log.jsonl"
    if not log.exists():
        return None
    entries = []
    for line in log.read_text(encoding="utf-8").splitlines():
        line = line.strip()
        if line:
            try:
                entries.append(json.loads(line))
            except json.JSONDecodeError:
                pass
    return entries


def parse_doctor_report(proc, result, label):
    try:
        report = json.loads((proc.stdout or "").strip())
    except json.JSONDecodeError as exc:
        result.fail(
            f"{label}: doctor --json did not emit parseable JSON: {exc} "
            f"(stdout: {(proc.stdout or '').strip()[:200]!r}, "
            f"stderr: {(proc.stderr or '').strip()[:200]!r})"
        )
        return None
    if not isinstance(report, dict):
        result.fail(f"{label}: doctor --json output is not a JSON object")
        return None
    if not isinstance(report.get("host"), str) or not isinstance(
        report.get("path"), str
    ):
        result.fail(f"{label}: doctor JSON lacks string host/path fields")
    findings = report.get("findings")
    if not isinstance(findings, list) or not findings:
        result.fail(f"{label}: doctor JSON has no findings array")
        return report
    for finding in findings:
        if not isinstance(finding, dict):
            result.fail(f"{label}: finding is not an object: {finding!r}")
            continue
        if not isinstance(finding.get("id"), str):
            result.fail(f"{label}: finding lacks string id: {finding!r}")
        status = finding.get("status")
        if status not in DOCTOR_STATUSES:
            result.fail(
                f"{label}: finding {finding.get('id')!r} has status "
                f"{status!r}, outside the agreed vocabulary "
                f"{sorted(DOCTOR_STATUSES)}"
            )
    return report


def finding_by_id(report, finding_id):
    for finding in report.get("findings", []):
        if isinstance(finding, dict) and finding.get("id") == finding_id:
            return finding
    return None


def assert_trust_not_observed(report, result, label):
    trust = finding_by_id(report, TRUST_FINDING_ID)
    if trust is None:
        result.fail(
            f"{label}: no {TRUST_FINDING_ID} finding; doctor must always "
            "report host trust as not_observed, never observed"
        )
    elif trust.get("status") != "not_observed":
        result.fail(
            f"{label}: {TRUST_FINDING_ID} status is "
            f"{trust.get('status')!r}; doctor never observes live host "
            "trust, so it must be not_observed"
        )
