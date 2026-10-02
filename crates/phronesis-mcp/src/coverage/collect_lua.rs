//! Per-test lcov collection for Lua under busted and luacov: the emitted
//! collection script (one isolated coverage run per graph test id,
//! `TN:`-tagged lcov files, the F2 manifest, and a zero-match guard —
//! PLAN.md Task K2).
//!
//! The busted flags and summary shape are pinned from busted's own source
//! and docs (busted is not installed on this machine — probe-first debt):
//! `--coverage` requires luacov, `--filter=PATTERN` is a Lua pattern
//! matched against the space-joined full name; names here are escaped and
//! anchored to select a literal full name. The
//! the plain summary line is `<N> success(es) / <N> failure(s) / <N>
//! error(s) / <N> pending(s) : <time> seconds`. Redirecting stdout turns
//! busted's colors off; the guard requires one complete successful summary.
//! luacov merges `luacov.stats.out` across runs, so the script deletes it
//! before each entry — that deletion is what isolates one test's hits.

use std::path::Path;

/// The evidence tool string imports carry: producer + runner, so `select`
/// never has to re-detect the runner.
pub const TOOL_LUACOV_BUSTED: &str = "luacov+busted";

pub(crate) fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Busted filters are Lua patterns, not literal titles.
pub(crate) fn literal_pattern(name: &str) -> String {
    let mut pattern = String::from("^");
    for character in name.chars() {
        if "^$()%.[]*+-?".contains(character) {
            pattern.push('%');
        }
        pattern.push(character);
    }
    pattern.push('$');
    pattern
}

/// The isolated collection script: one busted invocation with coverage per
/// entry (`(graph test id, spec file, runner-native test name)`), each run
/// writing `luacov.stats.out` (deleted first so hits do not accumulate
/// across entries), reporting through `luacov -r lcov` into
/// `luacov.report.out`, then prepending the `TN:`/`# node:` lines and
/// renaming to `<stem>.lcov`. The guard fails the entry loudly when the
/// filter matched anything other than one successful test; ends with the F2 manifest (written under
/// lua, which the runner guarantees) and the import command.
pub fn collection_script(entries: &[(String, String, String)], out_dir: &Path) -> String {
    let mut s = String::from("#!/bin/sh\nset -eu\nOUT=");
    s.push_str(&quote(&out_dir.to_string_lossy()));
    s.push_str("\nmkdir -p \"$OUT\"\nrm -f \"$OUT/manifest.json\" \"$OUT\"/*.lcov \"$OUT\"/*.info\nbusted --version\ncommand -v luacov >/dev/null\nn=0\n");
    let mut stems = crate::coverage::pytest::ReportStemAllocator::default();
    for (index, (id, file, name)) in entries.iter().enumerate() {
        let n = index + 1;
        // Same safe-stem rule as the pytest collector.
        let stem = stems.allocate(id);
        s.push_str("n=$((n+1))\n");
        // luacov merges stats across runs; deleting the stats file is what
        // makes this entry's hits and only this entry's.
        s.push_str("rm -f luacov.stats.out luacov.report.out\n");
        s.push_str(&format!(
            "busted --coverage --output=plainTerminal --lang=en --filter={} {} > \"$OUT/{n}.log\" 2>&1\n",
            quote(&literal_pattern(name)),
            quote(file)
        ));
        s.push_str(&format!(
            "test \"$(grep -Ec '^[0-9]+ success(es)? / ' \"$OUT/{n}.log\")\" = 1 && grep -Eq '^1 success / 0 failures / 0 errors / 0 pending : [0-9]+([.][0-9]+)? seconds$' \"$OUT/{n}.log\" || {{ echo {} >&2; exit 1; }}\n",
            quote(&format!("entry {n} must execute exactly one successful test: {name}"))
        ));
        s.push_str("test -s luacov.stats.out\nluacov -r lcov\ntest -s luacov.report.out\n");
        s.push_str(&format!(
            "printf '%s\\n' {} {} | cat - luacov.report.out > \"$OUT/{n}.tmp\"\nmv \"$OUT/{n}.tmp\" \"$OUT/{stem}.lcov\"\n",
            quote(&format!("TN:{id}")),
            quote(&format!("# node: busted --filter {name} {file}"))
        ));
        s.push_str("rm -f luacov.stats.out luacov.report.out\n");
    }
    s.push_str(MANIFEST_LUA);
    s.push_str(&format!(
        "echo {}\n",
        quote(&format!(
            "phr-mcp coverage import --format lcov-dir --tool {TOOL_LUACOV_BUSTED} {}",
            out_dir.display()
        ))
    ));
    s
}

/// The F2 manifest step, run under lua (a project running busted has an
/// interpreter by definition): HEAD revision plus SHA-256 digests of every
/// covered source, plus the `"runner"` key. Verified live against lua 5.5:
/// the digests match `shasum -a 256`, symlinked roots (macOS `/tmp`)
/// resolve through `pwd -P`, and a repo without HEAD fails loudly.
const MANIFEST_LUA: &str = r#"lua - "$OUT" <<'LUA'
local out = arg[1]
local function shell_quote(s)
  return "'" .. s:gsub("'", "'\\''") .. "'"
end
local function capture(cmd, must_succeed)
  local p = io.popen(cmd)
  if p == nil then error('cannot run: ' .. cmd) end
  local s = p:read('*a')
  local ok = p:close()
  if must_succeed and not ok then error('command failed: ' .. cmd) end
  return s or ''
end
local function trim(s)
  return (s:gsub('^%s+', ''):gsub('%s+$', ''))
end
local root = trim(capture('git rev-parse --show-toplevel', true))
-- Physical root, symlinks resolved (macOS /tmp, container workspaces).
root = trim(capture('cd ' .. shell_quote(root) .. ' && pwd -P', true))
local function digest(path)
  local line = capture('sha256sum ' .. shell_quote(path) .. ' 2>/dev/null || shasum -a 256 ' .. shell_quote(path), false)
  return line:match('^%x+')
end
local function normalize(p)
  local abs = p
  if p:sub(1, 1) ~= '/' then
    abs = root .. '/' .. p
  end
  local dir = abs:match('^(.*)/[^/]+$') or root
  local physical = trim(capture('cd ' .. shell_quote(dir) .. ' && pwd -P', true))
  local base = abs:match('[^/]+$') or ''
  local resolved = physical .. '/' .. base
  local prefix = root .. '/'
  if resolved:sub(1, #prefix) ~= prefix then
    error('source outside git root: ' .. p)
  end
  return resolved:sub(#prefix + 1)
end
local files = {}
local names = capture('ls -1 ' .. shell_quote(out), true)
for name in names:gmatch('[^\n]+') do
  if name:match('%.lcov$') or name:match('%.info$') then
    local f = io.open(out .. '/' .. name, 'r')
    if f then
      for line in f:lines() do
        local sf = line:match('^SF:(.*)$')
        if sf then
          sf = trim(sf)
          local rel = normalize(sf)
          local d = digest(root .. '/' .. rel)
          if d then
            files[rel] = d
          else
            error('cannot hash source ' .. sf)
          end
        end
      end
      f:close()
    end
  end
end
local keys = {}
for k in pairs(files) do keys[#keys + 1] = k end
table.sort(keys)
local function json_escape(s)
  return (s:gsub('\\', '\\\\'):gsub('"', '\\"'):gsub('\n', '\\n'):gsub('\t', '\\t'))
end
local parts = {}
for _, k in ipairs(keys) do
  parts[#parts + 1] = '    "' .. json_escape(k) .. '": "' .. files[k] .. '"'
end
local rev = trim(capture('git rev-parse HEAD', true))
local mf = io.open(out .. '/manifest.json', 'w')
mf:write('{\n  "revision": "', json_escape(rev), '",\n  "files": {\n')
mf:write(table.concat(parts, ',\n'))
mf:write('\n  },\n  "runner": "busted"\n}\n')
mf:close()
LUA
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn lua_script_isolates_each_test_and_fails_on_zero_matches() {
        let entries = vec![(
            "lua:myapp::spec::store_spec::store::loads the store".to_string(),
            "spec/store_spec.lua".to_string(),
            "store loads the store".to_string(),
        )];
        let s = collection_script(&entries, Path::new("/out"));
        assert!(s.starts_with("#!/bin/sh\nset -eu\nOUT="), "{s}");
        assert!(s.contains("command -v luacov >/dev/null"), "{s}");
        assert!(
            s.contains(
                "busted --coverage --output=plainTerminal --lang=en --filter='^store loads the store$' 'spec/store_spec.lua' > \"$OUT/1.log\" 2>&1"
            ),
            "{s}"
        );
        // Pinned from busted's plainTerminal summary format
        // (`1 success / 0 failures / 0 errors / 0 pending : … seconds`);
        // require the entire one-test summary, with no skipped/pending cases.
        // busted is not installed on this machine, so a real run must
        // confirm the real runner (probe-first debt, PLAN.md K decision 2).
        assert!(
            s.contains("^1 success / 0 failures / 0 errors / 0 pending"),
            "{s}"
        );
        assert!(
            s.contains("entry 1 must execute exactly one successful test: store loads the store"),
            "{s}"
        );
        // luacov merges luacov.stats.out across runs: the per-entry
        // deletion is the isolation.
        assert!(
            s.contains("rm -f luacov.stats.out luacov.report.out\n"),
            "{s}"
        );
        assert!(s.contains("luacov -r lcov"), "{s}");
        assert!(
            s.contains("printf '%s\\n' 'TN:lua:myapp::spec::store_spec::store::loads the store' '# node: busted --filter store loads the store spec/store_spec.lua'"),
            "{s}"
        );
        assert!(s.contains("\"runner\": \"busted\""), "{s}");
        assert!(
            s.contains("phr-mcp coverage import --format lcov-dir --tool luacov+busted /out"),
            "{s}"
        );
    }

    #[test]
    fn lua_script_dedupes_stems_and_quotes_lua_patterns() {
        let entries = vec![
            (
                "lua:myapp::spec::store_spec::it one".to_string(),
                "spec/store_spec.lua".to_string(),
                "it one".to_string(),
            ),
            // A distinct graph id whose stem collides (space and `/` both
            // flatten to `_`), pytest-style: every lcov file the importer
            // sees keeps one unambiguous name.
            (
                "lua:myapp::spec::store_spec::it/one".to_string(),
                "spec/store_spec.lua".to_string(),
                "it/one".to_string(),
            ),
        ];
        let s = collection_script(&entries, Path::new("/tmp/cov dir"));
        assert!(
            s.contains("\"$OUT/lua_myapp__spec__store_spec__it_one.lcov\"")
                && s.contains("\"$OUT/lua_myapp__spec__store_spec__it_one_2.lcov\""),
            "{s}"
        );
        // The out dir is shell-quoted; a filter name with a quote survives
        // quoting; Lua pattern specials are escaped before shell quoting.
        assert!(s.contains("OUT='/tmp/cov dir'"), "{s}");
        let quoted = collection_script(
            &[(
                "lua:x::spec::s_spec::it's".to_string(),
                "spec/s_spec.lua".to_string(),
                "it's".to_string(),
            )],
            Path::new("/out"),
        );
        assert!(quoted.contains("--filter='^it'\\''s$'"), "{quoted}");
    }
    #[test]
    #[cfg(unix)]
    fn emitted_lua_script_rejects_mixed_counts_and_stale_reports() {
        use std::os::unix::fs::PermissionsExt;
        use std::process::Command;
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let bin = root.join("bin");
        let out = root.join("coverage");
        std::fs::create_dir(&bin).expect("bin");
        std::fs::create_dir(&out).expect("out");
        let fake = r#"#!/usr/bin/env python3
import os,pathlib,sys
tool=pathlib.Path(sys.argv[0]).name
mode=os.environ['FAKE_MODE']
if tool=='busted':
 if '--version' in sys.argv:
  print('fixture');sys.exit(0)
 assert '--filter=^store handles %.%[%]%(%)%+%-%*%?%%%$%^$' in sys.argv,sys.argv
 if mode!='missingstats': pathlib.Path('luacov.stats.out').write_text('fresh')
 count={'zero':0,'multiple':2,'eleven':11,'twentyone':21}.get(mode,1)
 pending=1 if mode=='pending' else 0
 if mode=='duplicate': print('1 success / 0 failures / 0 errors / 0 pending : 0.001 seconds')
 print(str(count)+' '+('success' if count==1 else 'successes')+' / 0 failures / 0 errors / '+str(pending)+' pending : 0.001 seconds')
elif tool=='luacov':
 if mode!='missingreport': pathlib.Path('luacov.report.out').write_text('SF:src/store.lua\nDA:1,1\nend_of_record\n')
else:
 sys.stdin.read()
 pathlib.Path(sys.argv[2],'manifest.json').write_text('{"runner":"busted"}')
"#;
        for tool in ["busted", "luacov", "lua"] {
            let file = bin.join(tool);
            std::fs::write(&file, fake).expect("fake tool");
            std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        }
        let script = root.join("collect.sh");
        std::fs::write(
            &script,
            collection_script(
                &[(
                    "lua:fixture::selected".into(),
                    "spec/store_spec.lua".into(),
                    "store handles .[]()+-*?%$^".into(),
                )],
                &out,
            ),
        )
        .expect("script");
        for mode in [
            "success",
            "zero",
            "multiple",
            "eleven",
            "twentyone",
            "pending",
            "duplicate",
            "missingstats",
            "missingreport",
        ] {
            std::fs::write(root.join("luacov.stats.out"), "stale").expect("stats");
            std::fs::write(root.join("luacov.report.out"), "stale").expect("report");
            std::fs::write(out.join("old.lcov"), "stale").expect("lcov");
            std::fs::write(out.join("manifest.json"), "stale").expect("manifest");
            let result = Command::new("sh")
                .arg(&script)
                .current_dir(root)
                .env(
                    "PATH",
                    format!("{}:{}", bin.display(), std::env::var("PATH").expect("PATH")),
                )
                .env("FAKE_MODE", mode)
                .output()
                .expect("script");
            assert_eq!(
                result.status.success(),
                mode == "success",
                "{mode}: {result:?}"
            );
            assert!(!out.join("old.lcov").exists());
            assert_eq!(out.join("manifest.json").exists(), mode == "success");
        }
    }
}
