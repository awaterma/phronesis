#!/usr/bin/env python3
"""Exercise real collection, import, and changed-body selection, without canned reports."""
import argparse
import json
from pathlib import Path
import shutil
import subprocess
import tempfile


def run(root, *args):
    result = subprocess.run(args, cwd=root, text=True, capture_output=True)
    if result.returncode:
        raise RuntimeError(f'{args!r}:\n{result.stdout}\n{result.stderr}')
    return result.stdout


def write(root, name, content):
    path = root / name
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(content)


def smoke(binary, runner, root):
    label = runner
    typescript = runner == 'vitest-ts'
    if typescript:
        runner = 'vitest'
    fixtures = Path(__file__).resolve().parents[1] / 'crates/phronesis-mcp/tests/fixtures'
    names = {'python': 'lcov/python-store', 'swift': 'lcov/swift-store', 'vitest': 'lcov/ts-store', 'jest': 'lcov/ts-store', 'node': 'lcov/ts-store', 'mvn': 'jacoco/java-store', 'gradle': 'jacoco/java-store', 'lua': 'lcov/lua-store'}
    shutil.copytree(fixtures / names[runner], root, dirs_exist_ok=True)
    shutil.rmtree(root / 'cov')
    sources = {'python': 'pkg/store.py', 'swift': 'Sources/Store/Store.swift', 'vitest': 'src/store.js', 'jest': 'src/store.js', 'node': 'src/store.js', 'mvn': 'core/src/main/java/com/x/Store.java', 'gradle': 'core/src/main/java/com/x/Store.java', 'lua': 'src/store.lua'}
    if typescript:
        sources[runner] = 'src/store.ts'
    if runner in ('vitest', 'jest', 'node'):
        (root / 'src/store.ts').unlink()
        shutil.rmtree(root / 'tests')
        dependencies = {'vitest': {'vitest': '2.1.9', '@vitest/coverage-v8': '2.1.9'}, 'jest': {'jest': '29.7.0'}, 'node': {'c8': '10.1.3'}}[runner]
        write(root, 'package.json', json.dumps({'name': 'example-app', 'version': '1.0.0', 'devDependencies': dependencies}))
        write(root, 'src/store.js', 'exports.load = function load() {\n  return 7;\n};\nexports.other = function other() {\n  return 9;\n};\n')
        setup = {'vitest': "import {test, describe, expect} from 'vitest';\nimport store from '../src/store.js';\n", 'jest': "const store = require('../src/store.js');\n", 'node': "const {test} = require('node:test');\nconst assert = require('node:assert/strict');\nconst store = require('../src/store.js');\n"}[runner]
        test_file = 'tests/store.test.ts' if typescript else 'tests/store.test.js'
        if typescript:
            (root / 'src/store.js').unlink()
            write(root, 'src/store.ts', 'export function load(): number {\n  return 7;\n}\nexport function other(): number {\n  return 9;\n}\n')
            setup = "import {test, describe, expect} from 'vitest';\nimport * as store from '../src/store.ts';\n"
        assertion = 'assert.equal(store.load(), 7)' if runner == 'node' else 'expect(store.load()).toBe(7)'
        other = 'assert.equal(store.other(), 9)' if runner == 'node' else 'expect(store.other()).toBe(9)'
        write(root, test_file, setup + f"test('testLoad', () => {{ {assertion}; }});\ntest('testOther', () => {{ {other}; }});\n")
        if runner != 'node':
            write(root, test_file, setup + f"describe('scope (a+b)::one', () => {{ test('testLoad', () => {{ {assertion}; }}); }});\ndescribe('scope other', () => {{ test('testLoad', () => {{ {assertion}; }}); }});\ntest('testOther', () => {{ {other}; }});\n")
        run(root, 'npm', 'install', '--no-audit', '--no-fund')
    elif runner == 'lua':
        write(root, 'src/store.lua', 'local M = {}\nfunction M.load()\n  return 7\nend\nfunction M.other()\n  return 9\nend\nreturn M\n')
        write(root, 'spec/store_spec.lua', "local store = require('store')\ndescribe('store', function()\n  it('testLoad', function() assert.are.equal(7, store.load()) end)\n  it('testOther', function() assert.are.equal(9, store.other()) end)\nend)\n")
        write(root, '.luacov', 'return { include = { "src/store$" } }\n')
    elif runner == 'swift':
        write(root, 'Sources/Store/Store.swift', 'public func load() -> Int {\n    return 7\n}\npublic func other() -> Int {\n    return 9\n}\n')
        write(root, 'Tests/StoreTests/StoreTests.swift', 'import XCTest\n@testable import Store\nfinal class StoreTests: XCTestCase {\n    func testLoad() { XCTAssertEqual(Store.load(), 7) }\n    func testOther() { XCTAssertEqual(Store.other(), 9) }\n}\n')
    elif runner in ('mvn', 'gradle'):
        write(root, 'core/src/main/java/com/x/Store.java', 'package com.x;\npublic class Store {\n  public int load() {\n    return 7;\n  }\n  public int other() {\n    return 9;\n  }\n}\n')
        write(root, 'core/src/test/java/com/x/StoreTest.java', 'package com.x;\nimport org.junit.Test;\nimport static org.junit.Assert.assertEquals;\npublic class StoreTest {\n  @Test public void testLoad() { assertEquals(7, new Store().load()); }\n  @Test public void testOther() { assertEquals(9, new Store().other()); }\n}\n')
        if runner == 'mvn':
            write(root, 'pom.xml', '<project><modelVersion>4.0.0</modelVersion><groupId>com.x</groupId><artifactId>example-app</artifactId><version>1</version><packaging>pom</packaging><modules><module>core</module></modules></project>')
            write(root, 'core/pom.xml', '<project><modelVersion>4.0.0</modelVersion><groupId>com.x</groupId><artifactId>core</artifactId><version>1</version><properties><maven.compiler.source>17</maven.compiler.source><maven.compiler.target>17</maven.compiler.target></properties><dependencies><dependency><groupId>junit</groupId><artifactId>junit</artifactId><version>4.13.2</version><scope>test</scope></dependency></dependencies><build><plugins><plugin><groupId>org.apache.maven.plugins</groupId><artifactId>maven-surefire-plugin</artifactId><version>3.5.2</version></plugin><plugin><groupId>org.jacoco</groupId><artifactId>jacoco-maven-plugin</artifactId><version>0.8.12</version></plugin></plugins></build></project>')
        else:
            write(root, 'settings.gradle', "rootProject.name = 'example-app'\ninclude 'core'\n")
            write(root, 'build.gradle', '')
            write(root, 'core/build.gradle', "plugins { id 'java'; id 'jacoco' }\nrepositories { mavenCentral() }\ndependencies { testImplementation 'junit:junit:4.13.2' }\n")
    elif runner == 'python':
        write(root, 'pkg/__init__.py', '')
        write(root, 'pkg/store.py', 'def load():\n    return 7\n\ndef other():\n    return 9\n')
        write(root, 'tests/test_store.py', 'from pkg.store import load, other\ndef test_load():\n    assert load() == 7\ndef test_other():\n    assert other() == 9\n')
    write(root, '.gitignore', 'node_modules/\n.build/\ntarget/\n.gradle/\nbuild/\ncore/build/\n.phronesis/\n')
    run(root, 'git', 'init', '-q')
    run(root, binary, 'init', '--packs', 'none')
    run(root, 'git', 'add', '.')
    run(root, 'git', '-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid', 'commit', '-qm', 'fixture')
    run(root, binary, 'graph', 'rebuild')
    collector = {'python': 'pytest-cov', 'swift': 'swift-cov', 'mvn': 'java-cov', 'gradle': 'java-cov', 'lua': 'lua-cov'}.get(runner, 'js-cov')
    tool = {'python': 'coverage.py', 'swift': 'swift-cov', 'mvn': 'jacoco+mvn', 'gradle': 'jacoco+gradle', 'vitest': 'c8+vitest', 'jest': 'istanbul+jest', 'node': 'c8+node', 'lua': 'luacov+busted'}[runner]
    args = [binary, 'coverage', 'collect', '--tool', collector]
    if collector in ('js-cov', 'java-cov'):
        args += ['--runner', runner]
    source = sources[runner]
    edges = [json.loads(line) for line in (root / '.phronesis/graph.jsonl').read_text().splitlines()]
    expected_load = {e['a'][1] for e in edges if e['p'] == 'defines_test' and e['a'][1].endswith(('testLoad', 'test_load'))}
    expected_other = {e['a'][1] for e in edges if e['p'] == 'defines_test' and e['a'][1].endswith(('testOther', 'test_other'))}
    assert len(expected_load) == (2 if runner in ('vitest', 'jest') else 1), edges
    assert len(expected_other) == 1, edges
    # Repeat in fresh directories; each collector's unit tests check used-output handling.
    for attempt in range(2):
        out = root / '.phronesis' / f'smoke-{attempt}'
        run(root, *args, '--out', str(out))
        fmt = 'jacoco-dir' if collector == 'java-cov' else 'lcov-dir'
        run(root, binary, 'coverage', 'import', '--format', fmt, '--tool', tool, str(out))
        hits = [json.loads(line) for line in (root / '.phronesis/coverage.jsonl').read_text().splitlines()]
        load = [h for h in hits if h.get('file') == source and h.get('region', '').endswith('::load')]
        assert load, (runner, hits)
        assert {h['test'] for h in load} == expected_load, (runner, load)
        other = [h for h in hits if h.get('file') == source and h.get('region', '').endswith('::other')]
        assert other, (runner, hits)
        assert {h['test'] for h in other} == expected_other, (runner, other)
    path = root / source
    text = path.read_text()
    # Fixtures use a numeric body return; change that line without changing its identity.
    import re
    changed, count = re.subn(r'return\s+\d+', 'return 8', text, count=1)
    assert count, text
    path.write_text(changed)
    selected = json.loads(run(root, binary, 'coverage', 'select', '--json'))
    tests = [t for t in selected['tests'] if t['evidence'] == 'coverage_observation']
    assert {t['test'] for t in tests} == expected_load, selected
    assert all(t['command'] for t in tests), selected
    path.write_text(text)
    for test in tests:
        run(root, 'sh', '-c', test['command'])
    print(f'PASS {label}: real isolated collection, import, changed-body selection')


def node_collision_smoke(binary, root):
    """Three lossy stems, plus late failures, must preserve isolated identities."""
    titles = ['a+b', 'a?b', 'a_b_2']
    functions = ['load', 'other', 'third']
    write(root, 'src/store.js', ''.join(f'exports.{name} = function {name}() {{\n  return {number};\n}};\n' for name, number in zip(functions, [7, 9, 11])))
    write(root, 'tests/store.test.js', "const {test} = require('node:test');\nconst assert = require('node:assert/strict');\nconst store = require('../src/store.js');\n" + ''.join(f"test({title!r}, () => {{ assert.equal(store.{name}(), {number}); }});\n" for title, name, number in zip(titles, functions, [7, 9, 11])))
    run(root, 'git', 'add', 'src/store.js', 'tests/store.test.js')
    run(root, 'git', '-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid', 'commit', '-qm', 'collision fixture')
    run(root, binary, 'graph', 'rebuild')
    edges = [json.loads(line) for line in (root / '.phronesis/graph.jsonl').read_text().splitlines()]
    expected = {title: next(e['a'][1] for e in edges if e['p'] == 'defines_test' and e['a'][1].endswith('::' + title)) for title in titles}
    out = root / '.phronesis/collision-success'
    base = [binary, 'coverage', 'collect', '--tool', 'js-cov', '--runner', 'node']
    run(root, *base, '--out', str(out))
    reports = list(out.glob('*.lcov'))
    assert len(reports) == 3, reports
    assert {line[3:] for report in reports for line in report.read_text().splitlines() if line.startswith('TN:') and line[3:]} == set(expected.values())
    run(root, binary, 'coverage', 'import', '--format', 'lcov-dir', '--tool', 'c8+node', str(out))
    store = root / '.phronesis/coverage.jsonl'
    index = root / '.phronesis/coverage.index'
    previous = (store.read_bytes(), index.read_bytes())
    hits = [json.loads(line) for line in store.read_text().splitlines()]
    for title, name in zip(titles, functions):
        assert {hit['test'] for hit in hits if hit.get('region', '').endswith('::' + name)} == {expected[title]}, hits
    for mode in ['zero-match', 'missing-report', 'failed-test']:
        output = root / '.phronesis' / ('collision-' + mode)
        script = run(root, *base, '--out', str(output), '--emit-script')
        if mode == 'zero-match':
            # First entry remains genuine; the second runner must fail.
            script = script.replace("--test-name-pattern='^a\\?b$'", "--test-name-pattern='^absent$'")
        elif mode == 'missing-report':
            # The second runner passes, but its newly produced report is missing.
            needle = 'node - "$OUT/2.log"'
            assert needle in script, script
            script = script.replace(needle, 'rm -f "$OUT/2/lcov.info"\n' + needle, 1)
        else:
            original = (root / 'tests/store.test.js').read_text()
            changed = original.replace('assert.equal(store.other(), 9);', "throw new Error('late fixture failure');")
            assert changed != original
            (root / 'tests/store.test.js').write_text(changed)
        result = subprocess.run(['sh'], input=script, cwd=root, text=True, capture_output=True)
        if mode == 'failed-test':
            (root / 'tests/store.test.js').write_text(original)
        assert result.returncode, (mode, result.stdout, result.stderr)
        partial = list(output.glob('*.lcov'))
        assert len(partial) == 1, (mode, partial, script)
        assert not (output / 'manifest.json').exists(), mode
        rejected = subprocess.run([binary, 'coverage', 'import', '--format', 'lcov-dir', '--tool', 'c8+node', str(output)], cwd=root, text=True, capture_output=True)
        assert rejected.returncode, rejected
        assert previous == (store.read_bytes(), index.read_bytes()), 'partial collection changed prior evidence'
    print('PASS node collision: three distinct reports/identities; late zero-match, failed test and missing report preserve prior store')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', required=True)
    parser.add_argument('--runner', choices=['python', 'vitest', 'vitest-ts', 'jest', 'node', 'swift', 'mvn', 'gradle', 'lua'], required=True)
    parser.add_argument('--keep', action='store_true')
    options = parser.parse_args()
    root = Path(tempfile.mkdtemp(prefix='phr coverage smoke '))
    print(f'Smoke repository: {root}', flush=True)
    succeeded = False
    try:
        smoke(str(Path(options.binary).resolve()), options.runner, root)
        if options.runner == 'node':
            node_collision_smoke(str(Path(options.binary).resolve()), root)
        succeeded = True
    finally:
        if succeeded and not options.keep:
            shutil.rmtree(root)
