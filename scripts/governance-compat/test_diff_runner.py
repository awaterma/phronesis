"""Regression tests for diff_runner comparison fidelity (stdlib unittest).

Red-state contract: exit codes must be compared for every call even when the
call carries no `expect`, disk bytes for rules.json / log.jsonl /
emitted-capsules.json must be captured (raw + normalized) and compared, and
intermediate disk snapshots must be retained at each `expect_disk` step so a
later removal cannot hide a difference.
"""

import importlib.util
import os
import time
from unittest.mock import patch
import re
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent


def _load_runner():
    spec = importlib.util.spec_from_file_location(
        "diff_runner_under_test", HERE / "diff_runner.py")
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


dr = _load_runner()


def make_stub(directory: Path, body: str) -> str:
    directory.mkdir(parents=True, exist_ok=True)
    path = directory / "stub.sh"
    path.write_text("#!/bin/sh\n" + body)
    path.chmod(0o755)
    return str(path)


class ExitCodeComparisonTests(unittest.TestCase):
    def test_same_stdout_different_exit_is_a_mismatch(self):
        with tempfile.TemporaryDirectory() as td:
            base = Path(td) / "base"
            cand = Path(td) / "cand"
            stub_b = make_stub(base, "echo hi\nexit 0\n")
            stub_c = make_stub(cand, "echo hi\nexit 7\n")
            case = {"name": "exit-regression", "rules": None,
                    "calls": [{"argv": []}]}
            exit_key = dr.label_call(0, case["calls"][0]) + ".exit"
            _, nb, _, _ = dr.run_case_side(stub_b, case, base / "art")
            _, nc, _, _ = dr.run_case_side(stub_c, case, cand / "art")
            self.assertIn(exit_key, nb,
                          "normalized side must carry the exit code")
            self.assertIn(exit_key, nc)
            self.assertNotEqual(nb[exit_key], nc[exit_key])
            mismatches = [k for k, _, _ in dr.compare_artifacts(nb, nc)]
            self.assertIn(exit_key, mismatches)

    def test_raw_exit_artifact_retained_on_disk(self):
        with tempfile.TemporaryDirectory() as td:
            side = Path(td) / "art"
            stub = make_stub(Path(td), "echo hi\nexit 3\n")
            case = {"name": "exit-regression", "rules": None,
                    "calls": [{"argv": []}]}
            raw, _, _, _ = dr.run_case_side(stub, case, side)
            exit_key = dr.label_call(0, case["calls"][0]) + ".exit"
            self.assertEqual(raw[exit_key], "3")
            saved = side / re.sub(r"[^A-Za-z0-9._-]+", "__", exit_key)
            self.assertTrue(saved.exists())
            self.assertEqual(saved.read_text(), "3")


class DiskCaptureTests(unittest.TestCase):
    WRITER = (
        'printf \'{"rule_id":"keep-me"}\\n\' '
        '> "$PHRONESIS_PROJECT_ROOT/.phronesis/rules.json"\n'
        'printf \'{"rule_id":"log-a"}\\n\' '
        '> "$PHRONESIS_PROJECT_ROOT/.phronesis/log.jsonl"\n'
    )

    def test_disk_bytes_compared_not_just_substrings(self):
        with tempfile.TemporaryDirectory() as td:
            base = Path(td) / "base"
            cand = Path(td) / "cand"
            stub_b = make_stub(base, self.WRITER)
            stub_c = make_stub(
                cand, self.WRITER.replace("log-a", "log-altered-metadata"))
            case = {"name": "disk-regression", "rules": None,
                    "calls": [{"argv": []}]}
            rb, nb, _, _ = dr.run_case_side(stub_b, case, base / "art")
            rc, nc, _, _ = dr.run_case_side(stub_c, case, cand / "art")
            for side_norm in (nb, nc):
                self.assertIn("disk:.phronesis/log.jsonl", side_norm,
                              "disk bytes must be captured for comparison")
                self.assertIn("disk:.phronesis/rules.json", side_norm)
                self.assertIn("disk:.phronesis/emitted-capsules.json",
                              side_norm)
            self.assertEqual(nb["disk:.phronesis/log.jsonl"],
                             '{"rule_id":"log-a"}\n')
            mismatches = [k for k, _, _ in dr.compare_artifacts(nb, nc)]
            self.assertIn("disk:.phronesis/log.jsonl", mismatches)
            self.assertNotIn("disk:.phronesis/rules.json", mismatches)
            self.assertEqual(nb["disk:.phronesis/rules.json"],
                             nc["disk:.phronesis/rules.json"])
            for key in ("disk:.phronesis/log.jsonl",
                        "disk:.phronesis/rules.json"):
                self.assertIn(key, rb)
                self.assertIn(key, rc)
            self.assertNotEqual(rb["disk:.phronesis/log.jsonl"],
                                rc["disk:.phronesis/log.jsonl"])

    def test_intermediate_snapshot_survives_later_removal(self):
        body = (
            'case "$1" in\n'
            '  write) printf \'one\\n\' > '
            '"$PHRONESIS_PROJECT_ROOT/.phronesis/rules.json";;\n'
            '  remove) rm -f "$PHRONESIS_PROJECT_ROOT/.phronesis/rules.json";;\n'
            'esac\n'
            'exit 0\n'
        )
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            stub = make_stub(root, body)
            case = {
                "name": "snapshot-regression", "rules": None,
                "calls": [
                    {"argv": ["write"], "expect": {"exit": 0},
                     "expect_disk": [{"path": ".phronesis/rules.json",
                                      "contains": ["one"]}]},
                    {"argv": ["remove"], "expect": {"exit": 0}},
                ],
            }
            raw, norm, _, assertions = dr.run_case_side(stub, case, root / "art")
            self.assertEqual(assertions, [])
            self.assertEqual(norm["disk:.phronesis/rules.json"], "<absent>")
            snapshot = "disk:.phronesis/rules.json@call-00-write"
            self.assertIn(snapshot, raw, "intermediate snapshot missing")
            self.assertIn(snapshot, norm)
            self.assertEqual(raw[snapshot], "one\n")
            self.assertEqual(norm[snapshot], "one\n")


class NormalizerNegativeControls(unittest.TestCase):
    def test_literal_uuid_dates_and_rule_ids_survive_normalization(self):
        root = Path("/tmp/not-a-real-root")
        text = ('{"id":"r-1","msg":"ref 123e4567-e89b-12d3-a456-426614174000 '
                'filed 2026-10-30"}')
        out = dr.normalize(text, root)
        self.assertIn("123e4567-e89b-12d3-a456-426614174000", out)
        self.assertIn("2026-10-30", out)
        self.assertIn('"r-1"', out)

    def test_semantic_rule_id_and_message_changes_are_flagged(self):
        base = {"disk:.phronesis/rules.json":
                '{"rules":[{"id":"no-unwrap-in-src"}]}'}
        cand = {"disk:.phronesis/rules.json":
                '{"rules":[{"id":"no-unwrap-in-src-renamed"}]}'}
        self.assertEqual([k for k, _, _ in dr.compare_artifacts(base, cand)],
                         ["disk:.phronesis/rules.json"])
        base_msg = {"call-00.err": "warning: avoid unwrap()"}
        cand_msg = {"call-00.err": "warning: unwrap() is fine here"}
        self.assertEqual([k for k, _, _ in dr.compare_artifacts(base_msg, cand_msg)],
                         ["call-00.err"])

    def test_only_rmcp_tracing_timestamp_field_is_normalized(self):
        root = Path("/tmp/fixture")
        stamp = "2026-11-03T06:29:08.064120Z"
        message = stamp + "  WARN serve_inner: rmcp::service: message at " + stamp
        normalized = dr.normalize_mcp_stderr(message, root)
        self.assertEqual(normalized, "<TRACE_TS>  WARN serve_inner: rmcp::service: message at " + stamp)
        self.assertEqual(dr.normalize_mcp_stderr(stamp + " user diagnostic", root),
                         stamp + " user diagnostic")
        self.assertNotEqual(normalized, dr.normalize_mcp_stderr(message.replace("message", "changed"), root))

    def test_missing_side_is_flagged_not_treated_as_match(self):
        self.assertEqual([k for k, _, _ in dr.compare_artifacts(
            {dr.label_call(0, {"argv": []}) + ".exit": "0"}, {})],
            [dr.label_call(0, {"argv": []}) + ".exit"])


class RequiredCaseEnforcement(unittest.TestCase):
    def test_every_required_case_is_defined(self):
        names = {c["name"] for c in dr.CASES}
        self.assertEqual(set(dr.REQUIRED_CASES) - names, set())
        self.assertEqual(len(dr.REQUIRED_CASES), 23)


class NormalizedRetentionEndToEnd(unittest.TestCase):
    def test_normalized_copies_written_for_matching_keys_too(self):
        stub_body = (
            'case "$1" in\n'
            '  --help) echo "Usage phr-mcp"; exit 0;;\n'
            '  --version) echo "0.1"; exit 0;;\n'
            '  *) exit 1;;\n'
            'esac\n'
        )
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            stub = make_stub(root, stub_body)
            out_dir = root / "out"
            proc = subprocess.run(
                [sys.executable, str(HERE / "diff_runner.py"),
                 "--baseline-bin", stub, "--candidate-bin", stub,
                 "--output-dir", str(out_dir)],
                capture_output=True, text=True, timeout=300)
            self.assertEqual(proc.returncode, 3,
                             "stub cannot satisfy governance expectations, "
                             "so the runner must fail closed (exit 3)")
            norm_case = out_dir / "normalized" / "baseline" / "cli-help-version"
            files = sorted(p.name for p in norm_case.iterdir())
            self.assertTrue(files, "normalized copies must exist for all keys")
            self.assertTrue(any(f.endswith(".exit") for f in files),
                            "exit codes must be among normalized artifacts")
            self.assertEqual(
                len(files),
                len(sorted(p.name for p in
                           (out_dir / "baseline" / "cli-help-version").iterdir())),
                "normalized set must mirror the raw set")


class McpTransportTests(unittest.TestCase):
    def stub(self, root, before="", after="", response=""):
        path = root / "server.py"
        path.write_text(
            "#!" + sys.executable + "\n"
            "import sys, json, time\n" + before + "\n"
            "for line in sys.stdin:\n"
            "    msg = json.loads(line)\n"
            "    if 'id' not in msg: continue\n" + response +
            "    print(json.dumps({'jsonrpc':'2.0', 'id':msg['id'], 'result':{}}), flush=True)\n"
            + after + "\n")
        path.chmod(0o755)
        return str(path)

    def run_side(self, root, **kwargs):
        return dr.run_case_side(self.stub(root, **kwargs),
                                {"name": "transport", "rules": None, "mcp": []},
                                root / "artifacts")

    def test_stderr_and_natural_exit_differences_are_retained(self):
        with tempfile.TemporaryDirectory() as td:
            base, cand = Path(td) / "base", Path(td) / "cand"
            base.mkdir()
            cand.mkdir()
            rb, nb, pb, ab = self.run_side(base)
            rc, nc, pc, ac = self.run_side(
                cand, after="sys.stderr.write('late warning\\n')\nsys.exit(7)")
            self.assertEqual((pb, ab, pc), ([], [], []))
            self.assertTrue(ac)
            mismatches = {k for k, _, _ in dr.compare_artifacts(nb, nc)}
            self.assertEqual(mismatches, {"mcp-session.err", "mcp-session.exit"})
            self.assertEqual(rc["mcp-session.exit"], "7")
            self.assertEqual(rc["mcp-session.err"], "late warning\n")
            self.assertEqual((cand / "artifacts/mcp-session.err").read_text(), "late warning\n")

    def test_large_stderr_before_response_and_during_shutdown_is_drained(self):
        with tempfile.TemporaryDirectory() as td:
            raw, _, problems, assertions = self.run_side(
                Path(td), before="sys.stderr.write('a' * 1048576); sys.stderr.flush()",
                after="sys.stderr.write('b' * 1048576); sys.stderr.flush()")
            self.assertEqual((problems, assertions), ([], []))
            self.assertEqual(raw["mcp-session.err"], "a" * 1048576 + "b" * 1048576)

    def assert_call_times_out(self, response):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            conn = dr.McpConn(self.stub(root, response=response), root, os.environ.copy())
            conn.CALL_TIMEOUT = 0.15
            conn.CLOSE_TIMEOUT = 0.15
            started = time.monotonic()
            try:
                with self.assertRaisesRegex(RuntimeError, "timed out"):
                    conn.call("initialize", {})
                self.assertLess(time.monotonic() - started, 1.0)
            finally:
                conn.close()
            self.assertIsNotNone(conn.proc.poll())

    def test_partial_line_cannot_bypass_deadline(self):
        self.assert_call_times_out(
            "    sys.stdout.write('{'); sys.stdout.flush(); time.sleep(1.5)\n")

    def test_notifications_do_not_reset_call_deadline(self):
        self.assert_call_times_out(
            "    end = time.monotonic() + 1.5\n"
            "    while time.monotonic() < end:\n"
            "        print(json.dumps({'method':'progress'}), flush=True); time.sleep(.01)\n")

    def test_forced_shutdown_is_a_problem_and_child_is_reaped(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            conn = dr.McpConn(self.stub(root, after="time.sleep(1.5)"), root, os.environ.copy())
            conn.CLOSE_TIMEOUT = 0.15
            conn.call("initialize", {})
            self.assertTrue(conn.close())
            self.assertIsNotNone(conn.proc.poll())
            self.assertNotEqual(conn.proc.returncode, 0)

    def test_oversized_line_fails_closed(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            conn = dr.McpConn(self.stub(root, response=
                "    sys.stdout.write('x' * 4096); sys.stdout.flush()\n"), root, os.environ.copy())
            conn.MAX_LINE_BYTES = 1024
            try:
                with self.assertRaisesRegex(RuntimeError, "line limit"):
                    conn.call("initialize", {})
            finally:
                conn.close()

    def test_timeout_preserves_stderr_and_reports_failure(self):
        with tempfile.TemporaryDirectory() as td:
            with patch.object(dr.McpConn, "CALL_TIMEOUT", .15), patch.object(dr.McpConn, "CLOSE_TIMEOUT", .15):
                raw, _, problems, _ = self.run_side(
                    Path(td), before="sys.stderr.write('diagnostic\\n'); sys.stderr.flush()",
                    response="    sys.stdout.write('{'); sys.stdout.flush(); time.sleep(1.5)\n")
            self.assertTrue(problems)
            self.assertEqual(raw["mcp-session.err"], "diagnostic\n")


class DiskPositiveExpectationTests(unittest.TestCase):
    def test_missing_file_cannot_satisfy_mixed_positive_negative_assertions(self):
        with tempfile.TemporaryDirectory() as td:
            root = Path(td)
            spec = {"path": "missing", "contains": ["banner"], "not_contains": ["error"]}
            self.assertTrue(dr.check_disk_expect(spec, root))
            self.assertEqual(dr.check_disk_expect(
                {"path": "missing", "not_contains": ["error"]}, root), [])
            (root / "missing").write_text("banner")
            self.assertEqual(dr.check_disk_expect(spec, root), [])


if __name__ == "__main__":
    unittest.main()