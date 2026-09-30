//! Per-test lcov collection for vitest, jest, and `node --test` under c8:
//! runner detection from `package.json` and the emitted collection script
//! (one isolated coverage run per graph test id, `TN:`-tagged lcov files,
//! the F2 manifest, and a zero-match guard — PLAN.md Task H2).
//!
//! Every zero-match guard is pinned from a real run (plan decision 7):
//! vitest 2.1.9 and jest 29 both exit 0 when the test filter matches
//! nothing (`Tests 1 skipped`), so the guard greps the `1 passed` summary;
//! node 26 still reports `pass 1` for the file itself when the name
//! pattern matches zero tests, so its guard greps the per-test `✔` line.

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

fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
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
    let mut stems = std::collections::BTreeMap::<String, usize>::new();
    for (index, (id, file, name)) in entries.iter().enumerate() {
        let n = index + 1;
        // Same safe-stem rule as the pytest collector.
        let base = crate::coverage::pytest::file_stem_for(id);
        let count = stems
            .entry(base.clone())
            .and_modify(|c| *c += 1)
            .or_insert(1);
        let stem = if *count > 1 {
            format!("{base}_{}", *count)
        } else {
            base
        };
        s.push_str("n=$((n+1))\n");
        match runner {
            JsRunner::Vitest => {
                s.push_str(&format!(
                    "npx vitest run {} -t {} --coverage.enabled --coverage.provider=v8 --coverage.reporter=lcov --coverage.reportsDirectory=\"$OUT/{n}\" > \"$OUT/{n}.log\" 2>&1\n",
                    quote(file),
                    quote(name)
                ));
                s.push_str(&zero_match_guard(n, name, "'1 passed'"));
            }
            JsRunner::Jest => {
                s.push_str(&format!(
                    "npx jest {} -t {} --coverage --coverageReporters=lcov --coverageDirectory=\"$OUT/{n}\" > \"$OUT/{n}.log\" 2>&1\n",
                    quote(file),
                    quote(name)
                ));
                s.push_str(&zero_match_guard(n, name, "'1 passed'"));
            }
            JsRunner::NodeTest => {
                s.push_str(&format!(
                    "npx c8 -r lcov -o \"$OUT/{n}\" node --test --test-name-pattern={} {} > \"$OUT/{n}.log\" 2>&1\n",
                    quote(&format!("^{name}$")),
                    quote(file)
                ));
                // node still prints `pass 1` for the file when the pattern
                // matches zero tests, so the guard greps the per-test line.
                s.push_str(&zero_match_guard(n, name, &format!("-F '✔ {name}'")));
            }
        }
        let node_line = match runner {
            JsRunner::Vitest | JsRunner::Jest => format!("# node: {file} -t {name}"),
            JsRunner::NodeTest => format!("# node: {file} --test-name-pattern ^{name}$"),
        };
        s.push_str(&format!(
            "printf '%s\\n' {} {} | cat - \"$OUT/{n}/lcov.info\" > \"$OUT/{n}.tmp\" && mv \"$OUT/{n}.tmp\" \"$OUT/{stem}.lcov\"\n",
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

/// `grep -q <needle> "$OUT/<n>.log" || { echo 'entry <n> matched zero
/// tests: <name>' >&2; exit 1; }` — a filter that matched zero (or more
/// than the one) tests must fail the entry, never write a `TN:`-tagged
/// lcov with no (or mixed) hits.
fn zero_match_guard(n: usize, name: &str, needle: &str) -> String {
    format!(
        "grep -q {needle} \"$OUT/{n}.log\" || {{ echo {} >&2; exit 1; }}\n",
        quote(&format!("entry {n} matched zero tests: {name}"))
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
const os = require('os');
const path = require('path');
const cp = require('child_process');
const out = process.argv[2];
const root = cp.execSync('git rev-parse --show-toplevel', { encoding: 'utf8' }).trim();
const files = {};
for (const name of fs.readdirSync(out).sort()) {
  if (!name.endsWith('.lcov') && !name.endsWith('.info')) continue;
  for (const line of fs.readFileSync(path.join(out, name), 'utf8').split('\n')) {
    if (!line.startsWith('SF:')) continue;
    const p = os.path.realpathSync(line.slice(3).trim());
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
    use std::path::Path;

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
    fn vitest_script_isolates_each_test_and_fails_on_zero_matches() {
        let entries = vec![(
            "typescript:myapp::tests::store.test::Store loads".to_string(),
            "tests/store.test.ts".to_string(),
            "Store loads".to_string(),
        )];
        let s = collection_script(JsRunner::Vitest, &entries, Path::new("/out"));
        assert!(s.starts_with("#!/bin/sh\nset -eu\n"));
        assert!(s.contains("npx vitest --version"), "{s}");
        assert!(s.contains("npx vitest run 'tests/store.test.ts' -t 'Store loads' --coverage.enabled --coverage.provider=v8 --coverage.reporter=lcov --coverage.reportsDirectory=\"$OUT/1\""), "{s}");
        // Pinned from a real vitest 2.1.9 run: a `-t` filter that matches
        // zero tests exits 0 and reports `Tests  1 skipped (1)`, so this
        // grep is the only thing standing between a silent empty lcov and
        // a loud failure (plan decision 7, Review Focus 4).
        assert!(s.contains("grep -q '1 passed' \"$OUT/1.log\""), "{s}");
        assert!(s.contains("entry 1 matched zero tests: Store loads"), "{s}");
        assert!(s.contains("printf '%s\\n' 'TN:typescript:myapp::tests::store.test::Store loads' '# node: tests/store.test.ts -t Store loads'"), "{s}");
        assert!(
            s.contains("\"runner\": \"vitest\""),
            "manifest records the runner: {s}"
        );
        assert!(
            s.contains("phr-mcp coverage import --format lcov-dir --tool c8+vitest /out"),
            "{s}"
        );
    }

    #[test]
    fn jest_and_node_scripts_use_their_native_flags_and_guards() {
        let entries = vec![(
            "typescript:myapp::tests::store.test::Store loads".to_string(),
            "tests/store.test.ts".to_string(),
            "Store loads".to_string(),
        )];
        let jest = collection_script(JsRunner::Jest, &entries, Path::new("/out"));
        assert!(jest.contains("npx jest --version"), "{jest}");
        // Jest's `-t` zero-match shape was pinned live too: exit 0,
        // `Tests:  1 skipped, 1 total`.
        assert!(jest.contains("npx jest 'tests/store.test.ts' -t 'Store loads' --coverage --coverageReporters=lcov --coverageDirectory=\"$OUT/1\""), "{jest}");
        assert!(jest.contains("grep -q '1 passed' \"$OUT/1.log\""), "{jest}");
        assert!(jest.contains("\"runner\": \"jest\""), "{jest}");
        assert!(jest.contains("--tool istanbul+jest"), "{jest}");

        let node = collection_script(JsRunner::NodeTest, &entries, Path::new("/out"));
        assert!(node.contains("node --version"), "{node}");
        assert!(node.contains("npx c8 -r lcov -o \"$OUT/1\" node --test --test-name-pattern='^Store loads$' 'tests/store.test.ts'"), "{node}");
        // Pinned from a real node 26 run: with `--test-name-pattern`
        // matching zero tests the summary still says `pass 1` (the file
        // itself "passes"), so the guard greps the per-test line instead.
        assert!(node.contains("grep -q -F '✔ Store loads'"), "{node}");
        assert!(node.contains("\"runner\": \"node\""), "{node}");
        assert!(node.contains("--tool c8+node"), "{node}");
    }
}
