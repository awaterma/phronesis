use std::collections::BTreeSet;
use std::path::Path;

use crate::init::types::{InitError, InitOpts, InitReport, Pack};
use crate::outcomes::toolchain::ToolchainDef;

/// The toolchain defs each selected language pack ships, in pack-list
/// order. Adding a language pack is one arm here; the writer below never
/// changes.
fn language_pack_defs(packs: &[Pack]) -> Vec<ToolchainDef> {
    let mut defs = Vec::new();
    for pack in packs {
        if *pack == Pack::Lua {
            defs.extend(lua_pack_defs());
        }
    }
    defs
}

/// The lua pack's toolchain def. The summary regex is pinned from busted's
/// own `plainTerminal` format (`N successes / N failures / N errors / N
/// pending : … seconds`, singular at a count of one) — busted is not
/// installed on this machine, so a real run must confirm it (probe-first
/// debt, PLAN.md K decision 5).
fn lua_pack_defs() -> Vec<ToolchainDef> {
    vec![ToolchainDef {
        id: "busted".into(),
        matches: r"^busted(\s|$)".into(),
        compile_fail: vec![],
        compile_success: vec![],
        test_summary: Some(r"(?P<passed>\d+) successes? / (?P<failed>\d+) failures?".into()),
        per_test: None,
        pass_tokens: vec![],
        outcome_kind: None,
    }]
}

/// Merge the selected language packs' toolchain defs into
/// `.phronesis/toolchains.json`: create the file when absent, append only
/// missing ids when present, and rewrite nothing when every id already
/// exists. A malformed existing file is an error naming the file — never a
/// silent overwrite that would strand the user's entries.
pub(super) fn write_language_pack_toolchains(
    root: &Path,
    opts: &InitOpts,
    report: &mut InitReport,
) -> Result<(), InitError> {
    let defs = language_pack_defs(&opts.packs);
    if defs.is_empty() {
        return Ok(());
    }
    let path = root.join(".phronesis/toolchains.json");
    let mut merged: Vec<serde_json::Value> = Vec::new();
    let mut added: Vec<String> = Vec::new();
    if path.exists() {
        let text = std::fs::read_to_string(&path).map_err(|e| InitError::Io {
            path: path.display().to_string(),
            source: e,
        })?;
        merged = serde_json::from_str(&text).map_err(|e| InitError::Toolchains {
            path: path.display().to_string(),
            source: e,
        })?;
        let present: BTreeSet<String> = merged
            .iter()
            .filter_map(|d| d.get("id").and_then(|i| i.as_str()))
            .map(str::to_string)
            .collect();
        for def in &defs {
            if present.contains(def.id.as_str()) {
                continue;
            }
            added.push(def.id.clone());
            merged.push(serde_json::to_value(def).map_err(InitError::Json)?);
        }
    } else {
        for def in &defs {
            added.push(def.id.clone());
            merged.push(serde_json::to_value(def).map_err(InitError::Json)?);
        }
    }
    if added.is_empty() {
        report.steps.push(format!(
            "= .phronesis/toolchains.json already carries the language-pack defs ({}) — leaving unchanged",
            defs.iter().map(|d| d.id.as_str()).collect::<Vec<_>>().join(", ")
        ));
        return Ok(());
    }
    if opts.dry_run {
        report.steps.push(format!(
            "+ would add {} to .phronesis/toolchains.json",
            added.join(", ")
        ));
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| InitError::Io {
            path: parent.display().to_string(),
            source: e,
        })?;
    }
    let json = serde_json::to_string_pretty(&merged).map_err(InitError::Json)?;
    std::fs::write(&path, json + "\n").map_err(|e| InitError::Io {
        path: path.display().to_string(),
        source: e,
    })?;
    report.steps.push(format!(
        "+ added {} to .phronesis/toolchains.json",
        added.join(", ")
    ));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::init::types::InitOpts;
    use crate::outcomes::toolchain::{CompiledDef, DefSource};
    use std::path::PathBuf;

    fn opts(packs: &[Pack]) -> InitOpts {
        InitOpts {
            project_root: PathBuf::new(),
            packs: packs.to_vec(),
            force: false,
            dry_run: false,
            rules_only: false,
            hooks_only: false,
        }
    }

    fn ids(root: &std::path::Path) -> Vec<String> {
        let raw = std::fs::read_to_string(root.join(".phronesis/toolchains.json")).expect("read");
        let defs: Vec<serde_json::Value> = serde_json::from_str(&raw).expect("parse");
        defs.iter()
            .map(|d| d["id"].as_str().expect("id").to_string())
            .collect()
    }

    #[test]
    fn creates_toolchains_json_when_absent() {
        let dir = tempfile::tempdir().unwrap();
        let mut report = InitReport::default();
        write_language_pack_toolchains(dir.path(), &opts(&[Pack::Lua]), &mut report)
            .expect("write");
        let got = ids(dir.path());
        assert!(got.contains(&"busted".to_string()), "{got:?}");
        // The shipped def must compile like any project def: valid regexes
        // and the required named groups (summary `passed`). The summary
        // shape is pinned from busted's own plainTerminal format
        // (`N successes / N failures / N errors / N pending`); busted is
        // not installed on this machine, so a real run must confirm it
        // (probe-first debt, PLAN.md K decision 5).
        for def in language_pack_defs(&[Pack::Lua]) {
            CompiledDef::compile(def, DefSource::Project).expect("shipped def compiles");
        }
    }

    #[test]
    fn adds_missing_ids_and_leaves_user_entries_alone() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".phronesis")).unwrap();
        std::fs::write(
            dir.path().join(".phronesis/toolchains.json"),
            r#"[
  {"id": "busted", "matches": "user-edited"},
  {"id": "mine", "matches": "^mine(\\s|$)"}
]"#,
        )
        .unwrap();
        let mut report = InitReport::default();
        write_language_pack_toolchains(dir.path(), &opts(&[Pack::Lua]), &mut report)
            .expect("merge");
        let raw = std::fs::read_to_string(dir.path().join(".phronesis/toolchains.json")).unwrap();
        let defs: Vec<serde_json::Value> = serde_json::from_str(&raw).unwrap();
        let ids: Vec<&str> = defs.iter().filter_map(|d| d["id"].as_str()).collect();
        assert_eq!(ids, vec!["busted", "mine"], "no duplicate appended");
        assert_eq!(defs[0]["matches"], "user-edited");
        assert_eq!(
            defs[1]["matches"], "^mine(\\s|$)",
            "a non-language entry is untouched"
        );
    }

    #[test]
    fn no_language_packs_write_nothing_and_dry_run_never_touches_disk() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".phronesis")).unwrap();
        let mut report = InitReport::default();
        write_language_pack_toolchains(dir.path(), &opts(&[Pack::Rust]), &mut report)
            .expect("noop");
        assert!(!dir.path().join(".phronesis/toolchains.json").exists());
        let mut report = InitReport::default();
        let dry = InitOpts {
            dry_run: true,
            ..opts(&[Pack::Lua])
        };
        write_language_pack_toolchains(dir.path(), &dry, &mut report).expect("dry run");
        assert!(
            !dir.path().join(".phronesis/toolchains.json").exists(),
            "dry run writes nothing"
        );
        assert!(report.steps.iter().any(|s| s.contains("would add busted")));
    }
}
