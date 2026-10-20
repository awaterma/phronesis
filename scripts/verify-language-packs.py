#!/usr/bin/env python3
"""Independent real-Go language-pack smoke, with raw evidence for every step.

This script requires an already-built phr-mcp binary and a real Go toolchain.
It never treats an unavailable command or an expected failure as a skipped pass.
"""
import argparse
import json
import os
import select
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--artifacts', type=Path, required=True)
    parser.add_argument('--go', type=Path, required=True)
    args = parser.parse_args()
    binary = args.binary.resolve()
    go = args.go.resolve()
    out = args.artifacts.resolve()
    out.mkdir(parents=True, exist_ok=True)
    project = Path(tempfile.mkdtemp(prefix='phr-go-acceptance-'))
    report = {'binary': str(binary), 'go': str(go), 'project': str(project),
              'steps': [], 'passed': False}
    env = dict(os.environ, PHRONESIS_PROJECT_ROOT=str(project))

    def revision():
        p = subprocess.run(['git', 'rev-parse', 'HEAD'], cwd=project,
                           capture_output=True, text=True)
        return p.stdout.strip() if p.returncode == 0 else None

    def run(label, argv, *, input_text=None, expect=0, timeout=180):
        start = time.monotonic()
        try:
            p = subprocess.run([str(x) for x in argv], cwd=project, env=env,
                               input=input_text, capture_output=True, text=True,
                               timeout=timeout)
            code, stdout, stderr = p.returncode, p.stdout, p.stderr
        except (OSError, subprocess.TimeoutExpired) as exc:
            code, stdout, stderr = None, '', repr(exc)
        entry = {'name': label, 'command': [str(x) for x in argv], 'cwd': str(project),
                 'revision': revision(), 'duration_ms': round((time.monotonic()-start)*1000),
                 'exit': code, 'expected_exit': expect, 'stdout': stdout, 'stderr': stderr}
        report['steps'].append(entry)
        (out / f'{len(report["steps"]):02d}-{label}.json').write_text(
            json.dumps(entry, indent=2) + '\n')
        if code != expect:
            raise AssertionError(f'{label}: expected exit {expect}, got {code}; see {out}')
        return entry

    def write(rel, contents):
        path = project / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(contents)

    def mcp_inventory():
        start = time.monotonic()
        transcript = []
        proc = subprocess.Popen([str(binary), 'serve'], cwd=project, env=env,
                                stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                stderr=subprocess.PIPE, text=True)
        try:
            for ident, method, params in [
                (1, 'initialize', {'protocolVersion': '2024-11-05', 'capabilities': {},
                                   'clientInfo': {'name': 'verify-language-packs', 'version': '1'}}),
                (2, 'tools/call', {'name': 'list_language_packs', 'arguments': {}}),
            ]:
                request = {'jsonrpc': '2.0', 'id': ident, 'method': method, 'params': params}
                proc.stdin.write(json.dumps(request) + '\n')
                proc.stdin.flush()
                ready, _, _ = select.select([proc.stdout], [], [], 120)
                if not ready:
                    raise TimeoutError(f'MCP did not reply to {method} within 120 s')
                line = proc.stdout.readline()
                if not line:
                    raise AssertionError(f'MCP closed before reply to {method}')
                transcript.append({'request': request, 'reply': json.loads(line)})
                if ident == 1:
                    proc.stdin.write(json.dumps({'jsonrpc': '2.0',
                        'method': 'notifications/initialized', 'params': {}}) + '\n')
                    proc.stdin.flush()
            payload = transcript[-1]['reply']['result']
            if payload.get('isError'):
                raise AssertionError(f'MCP inventory error: {payload}')
            structured = payload.get('structuredContent', {})
            if not any(p.get('id') == 'go' for p in structured.get('packs', [])):
                raise AssertionError(f'MCP inventory did not list Go: {structured}')
            entry = {'name': 'mcp-inventory', 'command': [str(binary), 'serve'],
                     'revision': revision(), 'duration_ms': round((time.monotonic()-start)*1000),
                     'exit': 0, 'transcript': transcript}
            report['steps'].append(entry)
            (out / f'{len(report["steps"]):02d}-mcp-inventory.json').write_text(
                json.dumps(entry, indent=2) + '\n')
        finally:
            proc.kill()
            proc.communicate(timeout=10)

    try:
        run('go-version', [go, 'version'])
        run('git-init', ['git', 'init', '-q'])
        run('git-config-name', ['git', 'config', 'user.name', 'Phronesis verifier'])
        run('git-config-email', ['git', 'config', 'user.email', 'verifier@example.invalid'])
        write('go.mod', 'module example.invalid/verify\n\ngo 1.26\n')
        write('calc.go', 'package verify\n\nfunc Add(a, b int) int { return a + b }\n')
        write('calc_test.go', 'package verify\n\nimport "testing"\n\nfunc TestAdd(t *testing.T) {\n    if Add(2, 3) != 5 { t.Fatal("bad sum") }\n}\n')
        run('init-go', [binary, 'init', '--packs', 'go'])
        run('git-add', ['git', 'add', 'go.mod', 'calc.go', 'calc_test.go'])
        run('git-commit', ['git', 'commit', '-qm', 'fixture'])
        run('graph-rebuild', [binary, 'graph', 'rebuild'])
        run('graph-status', [binary, 'graph', 'status', '--json'])
        assert 'TestAdd' in run('graph-tests', [binary, 'graph', 'query', 'defines_test', '--json'])['stdout']
        assert 'PASS' in run('go-test-pass', [go, 'test', '-v', './...'])['stdout']
        pass_output = out / 'go-test-pass.txt'
        pass_output.write_text(report['steps'][-1]['stdout'] + report['steps'][-1]['stderr'])
        tags = run('signal-pass', [binary, 'signal', 'ingest', '--command', 'go test -v ./...',
                                   '--output', pass_output, '--exit', '0'])['stdout']
        assert 'outcome:compile_ok' in tags and 'outcome:test_pass' in tags, tags
        mcp_inventory()
        # The optional policy is post/audit only: pre-check must allow this edit.
        dotted = 'package verify\n\nimport . "fmt"\n\nfunc Add(a, b int) int { Println(a, b); return a + b }\n'
        hook_input = json.dumps({'tool_name': 'Write', 'tool_input': {
            'file_path': str(project / 'calc.go'), 'content': dotted}})
        run('pre-check', [binary, 'pre-check'], input_text=hook_input)
        write('calc.go', dotted)
        post = run('post-check', [binary, 'post-check'], input_text=hook_input, expect=1)
        assert 'Dot import' in post['stderr'] + post['stdout']
        assert 'go.warn-dot-import' in run('audit', [binary, 'audit', '--json'])['stdout']
        run('graph-rebuild-edited', [binary, 'graph', 'rebuild'])
        selection = run('static-select', [binary, 'coverage', 'select', '--json'])['stdout']
        assert 'static_reach' in selection, 'edited Go function had no static test selection'
        assert 'coverage_observation' not in selection, 'static selection invented dynamic coverage'
        run('go-test-after-edit', [go, 'test', '-v', './...'])
        # Failing assertion is separate from a compiler error; neither is a pass.
        write('calc_test.go', 'package verify\nimport "testing"\nfunc TestAdd(t *testing.T) { t.Fatal("intentional failure") }\n')
        assert 'FAIL' in run('go-test-fail', [go, 'test', '-v', './...'], expect=1)['stdout']
        fail_output = out / 'go-test-fail.txt'
        fail_output.write_text(report['steps'][-1]['stdout'] + report['steps'][-1]['stderr'])
        tags = run('signal-fail', [binary, 'signal', 'ingest', '--command', 'go test -v ./...',
                                   '--output', fail_output, '--exit', '1'])['stdout']
        assert 'outcome:compile_ok' in tags and 'outcome:test_fail' in tags, tags
        assert 'outcome:test_pass' not in tags, tags
        # A test that exits the process reports nothing: earlier PASS lines
        # and a nonzero exit must not ground a test pass.
        write('calc_test.go', 'package verify\nimport ("os"; "testing")\n'
              'func TestAdd(t *testing.T) { if Add(2, 3) != 5 { t.Fatal("bad") } }\n'
              'func TestExit(t *testing.T) { os.Exit(3) }\n')
        exited = run('go-test-exit', [go, 'test', '-v', './...'], expect=1)
        exit_output = out / 'go-test-exit.txt'
        exit_output.write_text(exited['stdout'] + exited['stderr'])
        tags = run('signal-exit', [binary, 'signal', 'ingest', '--command', 'go test -v ./...',
                                   '--output', exit_output, '--exit', '1'])['stdout']
        assert 'outcome:compile_ok' in tags and 'outcome:test_' not in tags, tags
        write('calc.go', 'package verify\nfunc Add(a, b int) int { return missing + b }\n')
        compile_failure = run('go-compile-fail', [go, 'test', '-v', './...'], expect=1)
        assert 'undefined: missing' in compile_failure['stdout'] + compile_failure['stderr']
        compile_output = out / 'go-compile-fail.txt'
        compile_output.write_text(report['steps'][-1]['stdout'] + report['steps'][-1]['stderr'])
        tags = run('signal-compile-fail', [binary, 'signal', 'ingest', '--command',
                                           'go test -v ./...', '--output', compile_output,
                                           '--exit', '1'])['stdout']
        assert 'outcome:compile_error' in tags and 'outcome:test_' not in tags, tags
        report['passed'] = True
    except Exception as exc:
        report['error'] = repr(exc)
    finally:
        (out / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
        shutil.rmtree(project, ignore_errors=True)
    print(out / 'report.json')
    return 0 if report['passed'] else 1


if __name__ == '__main__':
    sys.exit(main())
