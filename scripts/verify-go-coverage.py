#!/usr/bin/env python3
"""Real Go per-test coverage -> LCOV -> Phronesis selection acceptance.

Requires Go and a built phr-mcp. Uses no downloaded Go modules. Preserves
commands, producer output, reports, manifests and selection in --artifacts.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import select
import subprocess
import tempfile

SOURCE = """package calc

type A struct{}
type B struct{}
func Hit() int {
    return 1
}
func Miss() int {
    return 2
}
func (a A) Read() int {
    return 3
}
func (b B) Read() int {
    return 4
}
func Read() int {
    return 5
}
"""
TEST = """package calc
import "testing"
func TestHit(t *testing.T) {
    if Hit() != 1 || (A{}).Read() != 3 || Read() != 5 { t.Fatal("incorrect result") }
}
func TestOther(t *testing.T) {
    if Miss() != 2 || (B{}).Read() != 4 { t.Fatal("incorrect result") }
}
"""


def run_acceptance(binary, go, artifacts):
    artifacts.mkdir(parents=True, exist_ok=True)
    # Retained on failure as well as success for inspection.
    root = Path(tempfile.mkdtemp(prefix="go-coverage-", dir=artifacts))
    env = dict(os.environ, GOTOOLCHAIN="local", GOPROXY="off", GOSUMDB="off")
    env.update(GIT_AUTHOR_NAME="Coverage fixture", GIT_AUTHOR_EMAIL="fixture@example.invalid",
               GIT_COMMITTER_NAME="Coverage fixture", GIT_COMMITTER_EMAIL="fixture@example.invalid")
    steps = []

    def run(label, args, expected=0):
        result = subprocess.run([str(a) for a in args], cwd=root, env=env,
                                capture_output=True, text=True, timeout=180)
        step = dict(name=label, command=[str(a) for a in args], exit=result.returncode,
                    stdout=result.stdout, stderr=result.stderr)
        steps.append(step)
        (artifacts / f"{len(steps):02d}-{label}.json").write_text(json.dumps(step, indent=2))
        if expected == 0 and result.returncode != 0:
            raise AssertionError(f"{label}: {result.stderr}")
        if expected != 0 and result.returncode == 0:
            raise AssertionError(f"{label}: unexpectedly accepted invalid evidence")
        return result.stdout

    try:
        run("go-version", [go, "version"])
        (root / "go.mod").write_text("module example.invalid/cov\n\ngo 1.26\n")
        (root / "calc.go").write_text(SOURCE)
        (root / "calc_test.go").write_text(TEST)
        run("init-go", [binary, "init", "--packs", "go"])
        run("git-init", ["git", "init", "-q"])
        run("git-add", ["git", "add", "go.mod", "calc.go", "calc_test.go"])
        run("git-commit", ["git", "commit", "-qm", "coverage fixture"])
        revision = run("revision", ["git", "rev-parse", "HEAD"]).strip()
        run("graph-before", [binary, "graph", "rebuild"])
        edges = [json.loads(line) for line in (root / ".phronesis/graph.jsonl").read_text().splitlines()]
        ids = [e["a"][1] for e in edges if e.get("p") == "defines_test" and e["a"][1].endswith("::TestHit")]
        assert len(set(ids)) == 1, ids
        test_id = ids[0]
        cov = root / "cov"
        cov.mkdir()
        produced = run("go-test-cover", [go, "test", "-json", "-count=1", "-run", "^TestHit$",
                                          "-coverprofile=" + str(cov / "profile.out"), "."])
        events = [json.loads(line) for line in produced.splitlines()]
        assert sum(e.get("Action") == "pass" and e.get("Test") == "TestHit" for e in events) == 1
        assert not any(e.get("Action") == "run" and e.get("Test") == "TestOther" for e in events)
        # Convert this single isolated test's statement counters to line hits.
        # Preserve zeros and use max for overlapping blocks, not function guesses.
        counters = {}
        profile = (cov / "profile.out").read_text().splitlines()
        assert profile[0] in {"mode: set", "mode: count", "mode: atomic"}
        for line in profile[1:]:
            match = re.fullmatch(r"example\.invalid/cov/calc\.go:(\d+)\.(\d+),(\d+)\.(\d+) (\d+) (\d+)", line)
            assert match, line
            start, _, end, end_col, _, count = map(int, match.groups())
            if end_col == 1 and end > start:
                end -= 1
            for row in range(start, end + 1):
                counters[row] = max(counters.get(row, 0), count)
        report = "TN:\nSF:workspace/calc.go\n" + "".join(f"DA:{row},{count}\n" for row, count in sorted(counters.items())) + "end_of_record\n"
        (cov / "coverage.dat").write_text(report)
        manifest = {"revision": revision, "files": {"calc.go": hashlib.sha256(SOURCE.encode()).hexdigest()}}
        (cov / "manifest.json").write_text(json.dumps(manifest))
        args = [binary, "coverage", "import", "--format", "lcov-dir", "--tool", "go-test",
                "--workspace-prefix", "workspace", str(cov)]
        run("empty-tn-refused", args, expected=1)
        run("import", args + ["--test-id", test_id])
        # Verify .dat participates in the exact same digest checks as .info.
        manifest["files"]["calc.go"] = "0" * 64
        (cov / "manifest.json").write_text(json.dumps(manifest))
        run("digest-refused", args + ["--test-id", test_id], expected=1)
        manifest["files"]["calc.go"] = hashlib.sha256(SOURCE.encode()).hexdigest()
        (cov / "manifest.json").write_text(json.dumps(manifest))
        updated = SOURCE
        for value in range(1, 6):
            updated = updated.replace(f"return {value}\n", f"return {value} + 0\n")
        (root / "calc.go").write_text(updated)
        run("graph-after", [binary, "graph", "rebuild"])
        selected = json.loads(run("selection", [binary, "coverage", "select", "--json"]))
        (artifacts / "selection.json").write_text(json.dumps(selected, indent=2))
        dynamic = [t for t in selected["tests"] if t["evidence"] == "coverage_observation"]
        assert len(dynamic) == 1 and dynamic[0]["test"] == test_id, selected
        covered = dynamic[0]["regions"]
        assert any(r.endswith("::Hit") for r in covered), covered
        assert any(r.endswith("::A::Read") for r in covered), covered
        gaps = selected["functions_without_dynamic_evidence"]
        assert len(gaps) == 2 and any(r.endswith("::Miss") for r in gaps) and any(r.endswith("::B::Read") for r in gaps), selected
        assert not selected["dynamic_coverage_unsupported"], selected
        static_hit = [t for t in selected["tests"] if t["evidence"] == "static_reach" and t["test"] == test_id]
        assert static_hit and not any(r.endswith("::B::Read") for t in static_hit for r in t["regions"]), selected
        assert dynamic[0]["command"] and "TestHit" in dynamic[0]["command"], selected
        run("selected-test", ["sh", "-c", dynamic[0]["command"]])
        # Verify the actual MCP surface shares the CLI's region/gap identities.
        with open(artifacts / "mcp-stderr.log", "w") as stderr:
            proc = subprocess.Popen([str(binary), "serve"], cwd=root,
                                    env=dict(env, PHRONESIS_PROJECT_ROOT=str(root)),
                                    stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                    stderr=stderr, text=True)
            transcript = []
            try:
                for ident, method, params in [
                    (1, "initialize", {"protocolVersion": "2024-11-05", "capabilities": {},
                                       "clientInfo": {"name": "go-coverage-verifier", "version": "1"}}),
                    (2, "tools/call", {"name": "select_tests", "arguments": {"limit": 0}}),
                ]:
                    proc.stdin.write(json.dumps({"jsonrpc": "2.0", "id": ident, "method": method, "params": params}) + "\n")
                    proc.stdin.flush()
                    ready, _, _ = select.select([proc.stdout], [], [], 120)
                    if not ready:
                        raise TimeoutError("MCP selection timed out")
                    reply = json.loads(proc.stdout.readline())
                    transcript.append(reply)
                    assert "error" not in reply, reply
                    if ident == 1:
                        proc.stdin.write(json.dumps({"jsonrpc": "2.0", "method": "notifications/initialized", "params": {}}) + "\n")
                        proc.stdin.flush()
                result = transcript[-1]["result"]
                assert not result.get("isError"), result
                payload = result["structuredContent"]
                assert payload["functions_without_dynamic_evidence"] == gaps, payload
                assert payload["tests"] == selected["tests"], payload
            finally:
                proc.terminate()
                proc.communicate(timeout=10)
                (artifacts / "mcp-selection.json").write_text(json.dumps(transcript, indent=2))

        (artifacts / "result.json").write_text(json.dumps({"passed": True, "steps": len(steps), "fixture": str(root), "test_id": test_id}, indent=2))
    except BaseException as error:
        (artifacts / "result.json").write_text(json.dumps({"passed": False, "steps": len(steps), "fixture": str(root), "error": str(error)}, indent=2))
        raise


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--go", default="go")
    parser.add_argument("--artifacts", required=True, type=Path)
    args = parser.parse_args()
    run_acceptance(args.binary.resolve(), args.go, args.artifacts.resolve())


if __name__ == "__main__":
    main()
