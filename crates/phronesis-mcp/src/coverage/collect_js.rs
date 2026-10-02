//! Per-test lcov collection for vitest, jest, and `node --test` under c8:
//! runner detection from `package.json` and the emitted collection script
//! (one isolated coverage run per graph test id, `TN:`-tagged lcov files,
//! the F2 manifest, and a zero-match guard — PLAN.md Task H2).
//!
//! Machine-readable results must identify exactly one passing test before
//! coverage is tagged. Ambiguous result identities fail closed.

use std::path::Path;

/// The JS test runner a project's `package.json` declares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsRunner {
    Vitest,
    Jest,
    NodeTest,
}

impl JsRunner {
    pub fn as_str(self) -> &'static str {
        match self {
            JsRunner::Vitest => "vitest",
            JsRunner::Jest => "jest",
            JsRunner::NodeTest => "node",
        }
    }

    /// The evidence tool string imports carry: producer + runner, so
    /// `select` never has to re-detect the runner.
    pub fn tool_string(self) -> &'static str {
        match self {
            JsRunner::Vitest => "c8+vitest",
            JsRunner::Jest => "istanbul+jest",
            JsRunner::NodeTest => "c8+node",
        }
    }
}

/// Detect the runner from `package.json`: vitest → jest → `node --test`,
/// each looked up in `devDependencies` then `dependencies`. A missing or
/// unparsable `package.json` is an error naming the file — never a silent
/// default (the caller can pass `--runner` to bypass detection).
pub fn detect_runner(root: &Path) -> anyhow::Result<JsRunner> {
    let path = root.join("package.json");
    let text = std::fs::read_to_string(&path).map_err(|e| {
        anyhow::anyhow!(
            "no readable package.json in {}: {e}; pass --runner explicitly",
            root.display()
        )
    })?;
    let pkg: serde_json::Value = serde_json::from_str(&text)
        .map_err(|e| anyhow::anyhow!("parsing package.json in {}: {e}", root.display()))?;
    let has = |name: &str| {
        ["devDependencies", "dependencies"]
            .iter()
            .any(|section| pkg.get(section).is_some_and(|s| s.get(name).is_some()))
    };
    if has("vitest") {
        Ok(JsRunner::Vitest)
    } else if has("jest") {
        Ok(JsRunner::Jest)
    } else {
        Ok(JsRunner::NodeTest)
    }
}

pub(crate) fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Escape a runner's JavaScript regular expression and select a literal title.
pub(crate) fn exact_pattern(title: &str) -> String {
    let mut pattern = String::from("^");
    for ch in title.chars() {
        if "\\^$.*+?()[]{}|".contains(ch) {
            pattern.push('\\');
        }
        pattern.push(ch);
    }
    pattern.push('$');
    pattern
}

/// The isolated collection script: one runner invocation with coverage per
/// entry (`(graph test id, test file, runner-native test name)`), each
/// writing `$OUT/<n>/lcov.info`; the guard fails the entry loudly when the
/// filter matched zero tests; the `TN:`/`# node:` lines are prepended and
/// the file renamed to `<stem>.lcov`. Ends with the F2 manifest and the
/// import command.
pub fn collection_script(
    runner: JsRunner,
    entries: &[(String, String, String)],
    out_dir: &Path,
) -> String {
    let mut s = String::from("#!/bin/sh\nset -eu\n");
    match runner {
        JsRunner::Vitest => s.push_str("npx vitest --version\n"),
        JsRunner::Jest => s.push_str("npx jest --version\n"),
        JsRunner::NodeTest => s.push_str("node --version\n"),
    }
    s.push_str("OUT=");
    s.push_str(&quote(&out_dir.to_string_lossy()));
    s.push_str("\nmkdir -p \"$OUT\"\nn=0\n");
    s.push_str("if [ -n \"$(find \"$OUT\" -type f -print -quit)\" ]; then echo 'coverage output must be empty; use a fresh directory' >&2; exit 1; fi\n");
    let mut stems = crate::coverage::pytest::ReportStemAllocator::default();
    for (index, (id, file, name)) in entries.iter().enumerate() {
        let n = index + 1;
        // Same safe-stem rule as the pytest collector.
        let stem = stems.allocate(id);
        s.push_str("n=$((n+1))\n");
        match runner {
            JsRunner::Vitest => {
                s.push_str(&format!(
                    "npx vitest run {} -t {} --coverage.enabled --coverage.provider=v8 --coverage.reporter=lcov --coverage.reportsDirectory=\"$OUT/{n}\" --reporter=json --outputFile=\"$OUT/{n}.json\" > \"$OUT/{n}.log\" 2>&1\n",
                    quote(file),
                    quote(&exact_pattern(name))
                ));
                s.push_str(&result_guard(n, name, false));
            }
            JsRunner::Jest => {
                s.push_str(&format!(
                    "npx jest --runTestsByPath {} -t {} --coverage --coverageReporters=lcov --coverageDirectory=\"$OUT/{n}\" --json --outputFile=\"$OUT/{n}.json\" > \"$OUT/{n}.log\" 2>&1\n",
                    quote(file),
                    quote(&exact_pattern(name))
                ));
                s.push_str(&result_guard(n, name, false));
            }
            JsRunner::NodeTest => {
                s.push_str(&format!(
                    "npx c8 -r lcov -o \"$OUT/{n}\" node --test --test-reporter=tap --test-name-pattern={} {} > \"$OUT/{n}.log\" 2>&1\n",
                    quote(&exact_pattern(name)),
                    quote(file)
                ));
                // TAP must identify the selected test as well as its count;
                // a synthetic passing file alone is insufficient.
                s.push_str(&result_guard(n, name, true));
            }
        }
        let node_line = match runner {
            JsRunner::Vitest | JsRunner::Jest => format!("# node: {file} -t {name}"),
            JsRunner::NodeTest => format!("# node: {file} --test-name-pattern ^{name}$"),
        };
        s.push_str(&format!(
            "test -s \"$OUT/{n}/lcov.info\"\nprintf '%s\\n' {} {} | cat - \"$OUT/{n}/lcov.info\" > \"$OUT/{n}.tmp\"\nmv \"$OUT/{n}.tmp\" \"$OUT/{stem}.lcov\"\n",
            quote(&format!("TN:{id}")),
            quote(&node_line)
        ));
        s.push_str(&format!("rm -rf \"$OUT/{n}\"\n"));
    }
    s.push_str(&manifest_heredoc(runner));
    s.push_str(&format!(
        "echo {}\n",
        quote(&format!(
            "phr-mcp coverage import --format lcov-dir --tool {} {}",
            runner.tool_string(),
            out_dir.display()
        ))
    ));
    s
}

fn result_guard(n: usize, name: &str, tap: bool) -> String {
    let validator = if tap {
        r#"const text = fs.readFileSync(process.argv[2], 'utf8');
const names = [...text.matchAll(/^ok \d+ - (.*)$/gm)].map(m => m[1]).filter(n => !/ # (?:SKIP|TODO)\b/i.test(n));
if (names.length !== 1 || names[0] !== process.argv[3] ||
    !/^# pass 1$/m.test(text) ||
    !/^# fail 0$/m.test(text)) throw new Error('expected exactly one passing test');"#
    } else {
        r#"const report = JSON.parse(fs.readFileSync(process.argv[2], 'utf8'));
const tests = report.testResults.flatMap(r => r.assertionResults);
const executed = tests.filter(t => !['pending', 'skipped', 'todo', 'disabled'].includes(t.status));
if (executed.length !== 1 || executed[0].status !== 'passed' ||
    executed[0].fullName !== process.argv[3] || report.numPassedTests !== 1 ||
    report.numFailedTests !== 0) throw new Error('expected exactly one passing test with the exact title');"#
    };
    let ext = if tap { "log" } else { "json" };
    format!(
        "node - \"$OUT/{n}.{ext}\" {} <<'CHECK'\nconst fs = require('fs');\n{validator}\nCHECK\n",
        quote(name)
    )
}

/// The F2 manifest step, run under node rather than python3: a JS
/// project's devcontainer has node by definition (the runner needs it),
/// while python3 is not guaranteed there. Same contract as the pytest
/// collector's manifest — HEAD revision plus SHA-256 digests of every
/// covered source — plus the `"runner"` key.
fn manifest_heredoc(runner: JsRunner) -> String {
    const TEMPLATE: &str = r#"node - "$OUT" <<'JS'
const crypto = require('crypto');
const fs = require('fs');
const path = require('path');
const cp = require('child_process');
const out = process.argv[2];
const root = cp.execSync('git rev-parse --show-toplevel', { encoding: 'utf8' }).trim();
const files = {};
for (const name of fs.readdirSync(out).sort()) {
  if (!name.endsWith('.lcov') && !name.endsWith('.info')) continue;
  for (const line of fs.readFileSync(path.join(out, name), 'utf8').split('\n')) {
    if (!line.startsWith('SF:')) continue;
    const p = fs.realpathSync(line.slice(3).trim());
    const rel = path.relative(root, p);
    if (rel.startsWith('..' + path.sep)) throw new Error('source outside git root: ' + p);
    files[rel.split(path.sep).join('/')] = crypto.createHash('sha256').update(fs.readFileSync(p)).digest('hex');
  }
}
const rev = cp.execSync('git rev-parse HEAD', { encoding: 'utf8' }).trim();
const manifest = { "revision": rev, "files": files, "runner": "__RUNNER__" };
fs.writeFileSync(path.join(out, 'manifest.json'), JSON.stringify(manifest, null, 2) + '\n');
JS
"#;
    TEMPLATE.replace("__RUNNER__", runner.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runner_detection_prefers_vitest_then_jest_then_node() {
        let d = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            d.path().join("package.json"),
            r#"{"devDependencies":{"vitest":"^2"}}"#,
        )
        .expect("w");
        assert!(matches!(
            detect_runner(d.path()).expect("vitest"),
            JsRunner::Vitest
        ));
        // A runtime dependency counts too — plenty of projects run vitest
        // from `dependencies`.
        std::fs::write(
            d.path().join("package.json"),
            r#"{"dependencies":{"vitest":"^2"}}"#,
        )
        .expect("w");
        assert!(matches!(
            detect_runner(d.path()).expect("vitest"),
            JsRunner::Vitest
        ));
        std::fs::write(
            d.path().join("package.json"),
            r#"{"devDependencies":{"jest":"^29"}}"#,
        )
        .expect("w");
        assert!(matches!(
            detect_runner(d.path()).expect("jest"),
            JsRunner::Jest
        ));
        std::fs::write(d.path().join("package.json"), r#"{"name":"x"}"#).expect("w");
        assert!(matches!(
            detect_runner(d.path()).expect("node"),
            JsRunner::NodeTest
        ));
        std::fs::remove_file(d.path().join("package.json")).expect("rm");
        assert!(
            detect_runner(d.path())
                .unwrap_err()
                .to_string()
                .contains("package.json")
        );
    }

    #[test]
    #[cfg(unix)]
    fn emitted_scripts_validate_actual_runner_results_and_refuse_stale_output() {
        use std::os::unix::fs::PermissionsExt;
        use std::process::Command;
        let title = "load (a+b) 'quoted' $x";
        for runner in [JsRunner::Vitest, JsRunner::Jest, JsRunner::NodeTest] {
            for (count, report_title) in [
                (0, title),
                (1, title),
                (2, title),
                (11, title),
                (21, title),
                (1, "describe scope load"),
            ] {
                let d = tempfile::tempdir().expect("tempdir");
                let bin = d.path().join("bin");
                std::fs::create_dir(&bin).expect("bin");
                let fake = bin.join("npx");
                std::fs::write(&fake, r#"#!/usr/bin/env node
const fs = require('fs');
const args = process.argv.slice(2);
if (args.includes('--version')) process.exit(0);
const expected = process.env.EXPECTED_PATTERN;
const actual = args.find(a => a.startsWith('--test-name-pattern='))?.split('=').slice(1).join('=') || args[args.indexOf('-t') + 1];
if (actual !== expected) throw new Error('unsafe or inexact selector: ' + actual);
let out = args.find(a => a.startsWith('--coverage.reportsDirectory=') || a.startsWith('--coverageDirectory='))?.split('=').slice(1).join('=');
if (!out) out = args[args.indexOf('-o') + 1];
fs.mkdirSync(out, {recursive: true});
fs.writeFileSync(out + '/lcov.info', 'SF:src.js\nDA:1,1\nend_of_record\n');
const count = Number(process.env.COUNT);
if (args.includes('c8')) {
 for(let i=0;i<count;i++) console.log('# Subtest: ' + process.env.TITLE + '\nok ' + (i+1) + ' - ' + process.env.TITLE);
 console.log('# tests ' + count + '\n# pass ' + count + '\n# fail 0');
} else {
 const report = args.find(a => a.startsWith('--outputFile=')).slice('--outputFile='.length);
 fs.writeFileSync(report, JSON.stringify({numPassedTests: count, numFailedTests:0, testResults:[{assertionResults: Array.from({length:count}, () => ({fullName: process.env.TITLE, status:'passed'}))}]}));
}
"#).expect("fake runner");
                std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755))
                    .expect("chmod");
                std::fs::write(d.path().join("src.js"), "function load() {}\n").expect("source");
                for args in [
                    vec!["init", "-q"],
                    vec!["add", "src.js"],
                    vec![
                        "-c",
                        "user.name=Test",
                        "-c",
                        "user.email=test@example.com",
                        "commit",
                        "-qm",
                        "fixture",
                    ],
                ] {
                    assert!(
                        Command::new("git")
                            .args(args)
                            .current_dir(d.path())
                            .status()
                            .expect("git")
                            .success()
                    );
                }
                let out = d.path().join("out");
                let script = d.path().join("collect.sh");
                std::fs::write(
                    &script,
                    collection_script(
                        runner,
                        &[("test:id".into(), "tests/store.test.js".into(), title.into())],
                        &out,
                    ),
                )
                .expect("script");
                let run = || {
                    Command::new("sh")
                        .arg(&script)
                        .current_dir(d.path())
                        .env(
                            "PATH",
                            format!("{}:{}", bin.display(), std::env::var("PATH").expect("path")),
                        )
                        .env("COUNT", count.to_string())
                        .env("TITLE", report_title)
                        .env("EXPECTED_PATTERN", exact_pattern(title))
                        .output()
                        .expect("execute script")
                };
                let result = run();
                assert_eq!(
                    result.status.success(),
                    count == 1 && report_title == title,
                    "{runner:?} count {count}: {}",
                    String::from_utf8_lossy(&result.stderr)
                );
                assert_eq!(
                    out.join("manifest.json").exists(),
                    count == 1 && report_title == title
                );
                if count == 1 && report_title == title {
                    let before = std::fs::read(out.join("manifest.json")).expect("manifest");
                    assert!(!run().status.success(), "reuse must fail");
                    assert_eq!(
                        std::fs::read(out.join("manifest.json")).expect("manifest"),
                        before
                    );
                }
            }
        }
    }
}
