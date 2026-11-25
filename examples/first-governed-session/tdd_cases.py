"""Execute the published TDD exercise; direct hook calls are not host evidence."""

import json
import os
import re
import shutil
from pathlib import Path

from tutorial_support import EXAMPLE_DIR, hook_event, run_command


def check_tdd_walkthrough(phr_mcp, project, env, result):
    fixture = EXAMPLE_DIR / "tdd-practice"
    if not (fixture / "Cargo.toml").is_file():
        result.fail("missing runnable TDD practice crate")
        return
    tutorial = EXAMPLE_DIR.parents[1] / "docs/tutorials/first-governed-session.md"
    text = tutorial.read_text(encoding="utf-8")
    blocks = {}
    for name, language in [
        ("setup", "sh"), ("tests", "rust"), ("test-command", "sh"),
        ("blocked", "rust"), ("green", "rust"),
    ]:
        match = re.search(
            rf"<!-- check: tdd-{name} -->\s*```{language}\n(.*?)```", text, re.S
        )
        if not match:
            result.fail(f"missing executable tutorial block: tdd-{name}")
            return
        blocks[name] = match[1]
    if shutil.which("cargo", path=env.get("PATH")) is None:
        result.fail("TDD tutorial requires cargo on PATH; install Rust 1.90 or newer")
        return
    checkout = project / "checkout"
    shutil.copytree(fixture, checkout / "examples/first-governed-session/tdd-practice")
    binary = Path(shutil.which(phr_mcp)).resolve()
    # Retain build caches outside the disposable practice tree, including on failure.
    target = Path(env.get("CARGO_TARGET_DIR", str(Path.home() / ".cache/phronesis-tutorial-target"))).resolve()
    child_env = dict(env, PATH=str(binary.parent) + os.pathsep + env.get("PATH", ""),
                     TMPDIR=str(project), CARGO_TARGET_DIR=str(target))
    setup = run_command(
        ["sh", "-eu", "-c", blocks["setup"] + '\nprintf "\\nTDD_PROJECT=%s\\n" "$PWD"\n'],
        checkout, env=child_env,
    )
    (project / "setup.log").write_text(setup.stdout + setup.stderr)
    location = re.search(r"^TDD_PROJECT=(.+)$", setup.stdout, re.M)
    if setup.returncode != 0 or not location:
        result.fail(f"documented TDD setup failed: {setup.stdout[-600:]} {setup.stderr[-600:]}")
        return
    practice = Path(location[1])
    if (checkout / ".claude").exists() or (checkout / ".phronesis").exists():
        result.fail("TDD setup initialized checkout instead of practice copy")
        return
    if not (practice / ".claude/settings.local.json").is_file():
        result.fail("TDD setup did not wire Claude hooks")
        return
    source = practice / "src/lib.rs"
    original = source.read_text()
    if (practice / "tests").exists():
        result.fail("practice must start without tests; the reader adds them first")
        return

    def proposal(path, content, label, expected):
        proc = run_command([phr_mcp, "pre-check"], practice,
                           input_text=json.dumps(hook_event("Write", path, content)), env=child_env)
        (project / f"{label}.log").write_text(proc.stdout + proc.stderr)
        if proc.returncode != expected:
            result.fail(f"{label}: expected hook exit {expected}, got {proc.returncode}: {proc.stderr[-600:]}")
            return False
        return True

    tests = practice / "tests/visits.rs"
    if not proposal(tests, blocks["tests"], "test-write", 0):
        return
    tests.parent.mkdir()
    tests.write_text(blocks["tests"])
    red = run_command(["sh", "-eu", "-c", blocks["test-command"]], practice, env=child_env)
    (project / "red.log").write_text(red.stdout + red.stderr)
    if red.returncode != 101 or "parses_valid_count ... FAILED" not in red.stdout:
        result.fail(f"expected failing parses_valid_count assertion, not setup/compiler failure: {red.stdout[-600:]} {red.stderr[-600:]}")
        return
    if "malformed_count_is_zero ... ok" not in red.stdout:
        result.fail("red run did not exercise the malformed-input requirement")
        return
    if source.read_text() != original:
        result.fail("production source changed before the failing test")
        return
    if not proposal(source, blocks["blocked"], "blocked-write", 2):
        return
    entries = [json.loads(line) for line in (practice / ".phronesis/log.jsonl").read_text().splitlines()]
    if not any(reason.get("rule") == "tutorial-no-unwrap-in-src"
               for entry in entries for reason in entry.get("blocked_by", [])):
        result.fail("blocked proposal lacked the expected structural rule evidence")
        return
    # The checker simulates a cooperating host: never apply a denied proposal.
    if source.read_text() != original:
        result.fail("blocked proposal changed the source")
        return
    if not proposal(source, blocks["green"], "green-write", 0):
        return
    # An unwrap mention in a comment must not trigger a structural invocation rule.
    if not proposal(source, blocks["green"] + "\n// .unwrap() is not called here.\n", "comment-write", 0):
        return
    source.write_text(blocks["green"])
    green = run_command(["sh", "-eu", "-c", blocks["test-command"]], practice, env=child_env)
    (project / "green.log").write_text(green.stdout + green.stderr)
    if green.returncode != 0 or "2 passed; 0 failed" not in green.stdout:
        result.fail(f"documented corrected implementation did not pass both tests: {green.stdout[-600:]} {green.stderr[-600:]}")
        return
    if tests.read_text() != blocks["tests"]:
        result.fail("tests changed while making the implementation green")
        return
    result.note("published setup + real cargo assertion failure + structural hook block + corrected implementation + 2 tests green; direct hook evidence only")
