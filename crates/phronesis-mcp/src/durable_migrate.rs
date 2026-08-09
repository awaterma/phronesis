//! Bring an existing `.phronesis/durable.md` up to the current shipped
//! template.
//!
//! `init` migrates byte-identical prior templates automatically and leaves
//! customized files alone. The explicit command remains available for dry-run
//! inspection and targeted migration.
//!
//! A file is rewritten only if it matches a known shipped template verbatim.
//! Anything else is treated as customized and left untouched.

use std::path::{Path, PathBuf};

use thiserror::Error;

/// The template shipped before `b9389fe`, carrying the participatory-governance
/// section that `kind_ceiling` dropped from every session render.
///
/// Generated from `git show b9389fe~1:crates/phronesis-mcp/src/init.rs`, not
/// retyped. 3292 bytes — `str::len()` counts bytes and this text contains
/// multi-byte UTF-8 (em dash, arrow), so its character count is smaller and is
/// the wrong unit to assert on.
pub(crate) const DURABLE_V1: &str = r#"# Durable Directives

Re-injected at every SessionStart and UserPromptSubmit by phronesis.
Contents do not fade with context-window compression.

## Drift discipline

Three heuristic tools surface the gap between prose guidance and
enforced rules. They are cheap, deterministic, and worth running
whenever the user asks about rules, memory, durable guidance, or
project conventions:

- `mcp__phronesis__get_claude_md_drift` — bullets in `CLAUDE.md`
  that no current rule covers. Candidates for rule porting or for
  marking "non-lintable by design."
- `mcp__phronesis__get_memory_drift` — entries in the auto-memory
  store that have no matching rule or `durable.md` paragraph.
  Actionable entries (named tool calls / commands) should become
  rules; ambient ones (project-shared prose) should be added here.
- `mcp__phronesis__get_wiki_drift` — ADR-style decisions under
  `.phronesis/wiki/decisions/` that no rule enforces. Decisions
  with explicit `enforces: [rule-id]` frontmatter resolve
  deterministically; others fall through to Jaccard matching.

Treat the output as a triage list, not authoritative ground truth —
the scoring is token-overlap Jaccard, no semantic match. When the
user says "remember X" or "make a rule for X", check drift first to
see whether the gap is real.

## Participatory governance

The model is both governed by rules and a participant in rule
evolution. Three workflows close the loop:

### Remember → decide → enforce

When the user says "remember X" or "make a rule for X":

1. Check drift tools — is the gap real?
2. Scaffold a decision: `phr-mcp decision new <slug>`
3. Fill in Context, Decision, Enforcement, Consequences
4. If enforceable (code-shape, command pattern):
   - Propose a rule using available predicates
     (`new_content_contains`, `file_path_matches`,
     `file_extension_is`, etc.)
   - Write it to `.phronesis/rules.json`
   - Wire `enforces: [rule-id]` in the decision frontmatter
5. If not enforceable (process, naming, social):
   - Note in Enforcement that no automated rule is possible
   - Offer to add prose guidance to this file instead
6. Ask the human to approve before committing

### Friction-driven proposals

When a rule blocks you 3+ times in the same session for the same
pattern, pause and assess:

- Use `get_action_log` with `only_nonzero_exit: true` to review
- If the rule scope is too broad (legitimate code keeps tripping
  it): propose a decision page that refines the scope — narrower
  `file_path_matches`, an exclusion, a predicate change. Present
  the proposal to the human.
- If you keep hitting it legitimately: the rule is working. Adjust
  your approach, don't propose weakening enforcement.

### Cross-session knowledge transfer

When you discover something significant — a bug pattern, a design
insight, a rollout lesson — consider writing a decision page. ADR
pages in `.phronesis/wiki/decisions/` travel with the repo and are
available to future sessions. This turns a session-local discovery
into durable project knowledge. Ask the human before writing —
not every insight warrants a formal decision.

## Project-specific guidance

(Add team-specific directives below. Anything written here is
re-read by the model every turn and so is safe from context-window
fade.)
"#;

/// The template shipped by `b9389fe`, before drift consolidation. 1075 bytes.
pub(crate) const DURABLE_V2: &str = r#"# Durable Directives

Rendered at SessionStart (and PostCompact). If `.phronesis/context.json`
is present, `.phronesis/kernel.md` carries the per-turn directives —
keep anything needed every turn there, not here. Budget is measured:
`phr-mcp context inspect --event session`.

## Drift discipline

Drift tools surface guidance that no rule enforces — `CLAUDE.md`
bullets (`get_claude_md_drift`), auto-memory entries
(`get_memory_drift`), and ADR decisions (`get_wiki_drift`). Run one
when the user asks about rules, memory, or project conventions, or
says "remember X" / "make a rule for X".

Scoring is token-overlap Jaccard with no semantic match, so output is
a triage list, not ground truth.

## Participatory governance

Rule-evolution workflows — decision scaffolding, friction-driven
proposals, cross-session knowledge transfer — are in
`docs/participatory-governance.md`. Read it when proposing or
refining a rule.

## Project-specific guidance

Add team directives below. Keep them short: this file competes with
the active-rule list for the session budget.
"#;

/// The compact template shipped by v0.25.1, while code drift was still a
/// registered placeholder. 1071 bytes.
pub(crate) const DURABLE_V3: &str = r#"# Durable Directives

Rendered at SessionStart (and PostCompact). If `.phronesis/context.json`
is present, `.phronesis/kernel.md` carries the per-turn directives —
keep anything needed every turn there, not here. Budget is measured:
`phr-mcp context inspect --event session`.

## Drift discipline

`get_drift(source)` surfaces guidance that no rule enforces — `source` is
`claude_md`, `memory`, `wiki`, `code`, or `all`. Run it when the user asks
about rules, memory, or project conventions, or says "remember X" / "make
a rule for X". `code` reports no-graph until rule-staleness lands.

Scoring is token-overlap Jaccard with no semantic match, so output is
a triage list, not ground truth.

## Participatory governance

Rule-evolution workflows — decision scaffolding, friction-driven
proposals, cross-session knowledge transfer — are in
`docs/participatory-governance.md`. Read it when proposing or
refining a rule.

## Project-specific guidance

Add team directives below. Keep them short: this file competes with
the active-rule list for the session budget.
"#;

fn known_prior_templates() -> [&'static str; 3] {
    [DURABLE_V1, DURABLE_V2, DURABLE_V3]
}

#[derive(Debug, PartialEq, Eq)]
pub enum Status {
    /// No file at all — nothing to migrate.
    Absent,
    /// Matches the current shipped template.
    Current,
    /// Matches a known prior shipped template verbatim.
    Stale { version: u8 },
    /// Does not match any shipped template — the operator has edited it.
    Customized,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Migrated,
    WouldMigrate,
    AlreadyCurrent,
    SkippedCustomized,
    SkippedAbsent,
}

#[derive(Debug, Error)]
pub enum MigrateError {
    #[error("failed to read {path}: {source}")]
    Read {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to write {path}: {source}")]
    Write {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error(
        "{path} already exists and holds different content — \
         move or delete it, then re-run; refusing to destroy a backup"
    )]
    BackupConflict { path: String },
}

pub fn durable_path(project_root: &Path) -> PathBuf {
    project_root.join(".phronesis").join("durable.md")
}

fn backup_path(path: &Path) -> PathBuf {
    path.with_extension("md.bak")
}

/// Classify a `durable.md` without writing anything.
pub fn inspect(path: &Path) -> Result<Status, MigrateError> {
    let body = match std::fs::read_to_string(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Status::Absent),
        Err(e) => {
            return Err(MigrateError::Read {
                path: path.display().to_string(),
                source: e,
            });
        }
    };

    if body == crate::init::DEFAULT_DURABLE_MD {
        return Ok(Status::Current);
    }
    for (i, prior) in known_prior_templates().iter().enumerate() {
        if body == *prior {
            let version = u8::try_from(i + 1).unwrap_or(1);
            return Ok(Status::Stale { version });
        }
    }
    Ok(Status::Customized)
}

/// Preserve `body` at `durable.md.bak`.
///
/// Three cases, because the command's one promise is that the old content
/// survives:
///
/// - no backup yet → write it;
/// - a backup already holding exactly `body` → the content is already
///   preserved, so this is a no-op rather than an error. Re-running a
///   half-finished migration must not be blocked by its own first attempt;
/// - a backup holding anything else → refuse. Overwriting it would destroy
///   the copy the operator is most likely to want, while the command reports
///   success.
fn preserve(path: &Path, body: &str) -> Result<(), MigrateError> {
    let backup = backup_path(path);
    match std::fs::read_to_string(&backup) {
        Ok(existing) if existing == body => Ok(()),
        Ok(_) => Err(MigrateError::BackupConflict {
            path: backup.display().to_string(),
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => std::fs::write(&backup, body)
            .map_err(|e| MigrateError::Write {
                path: backup.display().to_string(),
                source: e,
            }),
        Err(e) => Err(MigrateError::Read {
            path: backup.display().to_string(),
            source: e,
        }),
    }
}

/// Rewrite an unedited stale `durable.md` to the current template, preserving
/// the prior content at `durable.md.bak`.
pub fn migrate(path: &Path, dry_run: bool) -> Result<Outcome, MigrateError> {
    match inspect(path)? {
        Status::Absent => Ok(Outcome::SkippedAbsent),
        Status::Current => Ok(Outcome::AlreadyCurrent),
        Status::Customized => Ok(Outcome::SkippedCustomized),
        Status::Stale { .. } => {
            if dry_run {
                return Ok(Outcome::WouldMigrate);
            }
            let existing = std::fs::read_to_string(path).map_err(|e| MigrateError::Read {
                path: path.display().to_string(),
                source: e,
            })?;
            preserve(path, &existing)?;
            std::fs::write(path, crate::init::DEFAULT_DURABLE_MD).map_err(|e| {
                MigrateError::Write {
                    path: path.display().to_string(),
                    source: e,
                }
            })?;
            Ok(Outcome::Migrated)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_durable(root: &std::path::Path, body: &str) -> std::path::PathBuf {
        let dir = root.join(".phronesis");
        std::fs::create_dir_all(&dir).expect("mkdir");
        let path = dir.join("durable.md");
        std::fs::write(&path, body).expect("write");
        path
    }

    #[test]
    fn an_unedited_v1_file_is_migratable() {
        let d = tempfile::tempdir().expect("tempdir");
        let path = write_durable(d.path(), DURABLE_V1);
        assert!(matches!(inspect(&path), Ok(Status::Stale { .. })));
    }

    #[test]
    fn an_unedited_v2_file_is_migratable() {
        let d = tempfile::tempdir().expect("tempdir");
        let path = write_durable(d.path(), DURABLE_V2);
        assert!(matches!(inspect(&path), Ok(Status::Stale { .. })));
    }

    #[test]
    fn a_current_file_is_already_up_to_date() {
        let d = tempfile::tempdir().expect("tempdir");
        let path = write_durable(d.path(), crate::init::DEFAULT_DURABLE_MD);
        assert!(matches!(inspect(&path), Ok(Status::Current)));
    }

    #[test]
    fn an_edited_file_is_never_touched() {
        let d = tempfile::tempdir().expect("tempdir");
        let edited = format!("{DURABLE_V1}\n- our team rule: always foo\n");
        let path = write_durable(d.path(), &edited);

        assert!(matches!(inspect(&path), Ok(Status::Customized)));

        let outcome = migrate(&path, false).expect("migrate");
        assert!(matches!(outcome, Outcome::SkippedCustomized));
        assert_eq!(
            std::fs::read_to_string(&path).expect("read"),
            edited,
            "a customized file must survive byte-for-byte"
        );
    }

    #[test]
    fn migrating_writes_the_current_template_and_backs_up() {
        let d = tempfile::tempdir().expect("tempdir");
        let path = write_durable(d.path(), DURABLE_V1);

        let outcome = migrate(&path, false).expect("migrate");
        assert!(matches!(outcome, Outcome::Migrated));
        assert_eq!(
            std::fs::read_to_string(&path).expect("read"),
            crate::init::DEFAULT_DURABLE_MD
        );
        assert_eq!(
            std::fs::read_to_string(backup_path(&path)).expect("read backup"),
            DURABLE_V1,
            "the prior content must be recoverable"
        );
    }

    #[test]
    fn a_backup_holding_other_content_is_never_clobbered() {
        // The command's one promise is that the old content survives.
        // Overwriting a backup that already holds something else destroys
        // the copy the operator is most likely to want.
        let d = tempfile::tempdir().expect("tempdir");
        let path = write_durable(d.path(), DURABLE_V1);
        std::fs::write(backup_path(&path), "a precious earlier backup").expect("seed");

        let result = migrate(&path, false);
        assert!(
            matches!(result, Err(MigrateError::BackupConflict { .. })),
            "must refuse rather than overwrite: {result:?}"
        );
        assert_eq!(
            std::fs::read_to_string(backup_path(&path)).expect("read backup"),
            "a precious earlier backup",
            "the earlier backup must be untouched"
        );
        assert_eq!(
            std::fs::read_to_string(&path).expect("read"),
            DURABLE_V1,
            "a refused migration must leave durable.md alone"
        );
    }

    #[test]
    fn a_backup_already_holding_the_same_content_is_not_an_obstacle() {
        // Re-running after a half-finished migration must not be blocked by
        // its own first attempt: the content is already preserved, so there
        // is nothing to protect and nothing to rewrite.
        let d = tempfile::tempdir().expect("tempdir");
        let path = write_durable(d.path(), DURABLE_V1);
        std::fs::write(backup_path(&path), DURABLE_V1).expect("seed identical backup");

        let outcome = migrate(&path, false).expect("migrate");
        assert!(matches!(outcome, Outcome::Migrated));
        assert_eq!(
            std::fs::read_to_string(&path).expect("read"),
            crate::init::DEFAULT_DURABLE_MD
        );
        assert_eq!(
            std::fs::read_to_string(backup_path(&path)).expect("read backup"),
            DURABLE_V1
        );
    }

    #[test]
    fn migration_is_idempotent() {
        let d = tempfile::tempdir().expect("tempdir");
        let path = write_durable(d.path(), DURABLE_V1);
        migrate(&path, false).expect("first");
        let second = migrate(&path, false).expect("second");
        assert!(matches!(second, Outcome::AlreadyCurrent));
    }

    #[test]
    fn dry_run_writes_nothing() {
        let d = tempfile::tempdir().expect("tempdir");
        let path = write_durable(d.path(), DURABLE_V1);
        let outcome = migrate(&path, true).expect("dry run");
        assert!(matches!(outcome, Outcome::WouldMigrate));
        assert_eq!(std::fs::read_to_string(&path).expect("read"), DURABLE_V1);
        assert!(!backup_path(&path).exists());
    }

    #[test]
    fn embedded_history_matches_the_real_shipped_bytes() {
        // Without this, every other test compares the constants only against
        // themselves: a mis-pasted historical template still "migrates" fine
        // in tests while failing to recognise any real file in the wild.
        //
        // These are BYTES. str::len() counts bytes and both templates carry
        // multi-byte UTF-8 (em dash, arrow), so their character counts are
        // smaller and are the wrong unit. Verify with `wc -c`, or Python
        // len(text.encode("utf-8")) -- a bare Python len() gives characters
        // and previously produced two wrong figures here.
        assert_eq!(DURABLE_V1.len(), 3292, "V1 byte length drifted");
        assert_eq!(DURABLE_V2.len(), 1075, "V2 byte length drifted");
        assert!(
            DURABLE_V1.contains("### Cross-session knowledge transfer"),
            "V1 must contain the participatory-governance section"
        );
        assert!(
            DURABLE_V1.contains("get_claude_md_drift"),
            "V1 predates consolidation and must still name the old tools"
        );
        assert!(
            DURABLE_V2.contains("docs/participatory-governance.md"),
            "V2 is the shrunk template that points at the extracted doc"
        );
        assert!(
            !DURABLE_V2.contains("### Cross-session knowledge transfer"),
            "V2 must not still carry the extracted section"
        );
        assert_ne!(DURABLE_V1, DURABLE_V2);
    }

    #[test]
    fn a_missing_file_is_not_an_error() {
        let d = tempfile::tempdir().expect("tempdir");
        let path = d.path().join(".phronesis").join("durable.md");
        assert!(matches!(inspect(&path), Ok(Status::Absent)));
    }

    #[test]
    fn no_known_template_names_a_removed_drift_tool_in_the_current_one() {
        // V1 and V2 legitimately name the old tools — that is why they need
        // migrating. The *current* template must not.
        for gone in ["get_claude_md_drift", "get_memory_drift", "get_wiki_drift"] {
            assert!(
                !crate::init::DEFAULT_DURABLE_MD.contains(gone),
                "current template still names {gone}"
            );
        }
    }

    #[test]
    fn current_template_is_not_in_the_prior_archive() {
        // If the current template is also in the prior list, the prior list
        // is stale — a template was archived without removing it from the
        // current slot, or vice versa.
        for prior in known_prior_templates() {
            assert_ne!(
                crate::init::DEFAULT_DURABLE_MD,
                prior,
                "current template must not duplicate a prior template"
            );
        }
    }

    #[test]
    fn the_current_template_byte_length_is_pinned() {
        // If DEFAULT_DURABLE_MD changes, this test fails — the author must
        // archive the outgoing bytes as a new DURABLE_V{n} constant and add
        // it to known_prior_templates before updating this pin. This is the
        // enforcement that F1 identified as missing: without it, a template
        // change silently strands every unedited installation as
        // `Customized`, closing the migration path.
        let current_bytes = crate::init::DEFAULT_DURABLE_MD.len();
        assert_eq!(
            current_bytes, 1019,
            "DEFAULT_DURABLE_MD byte length changed to {current_bytes} — \
             archive the outgoing template as DURABLE_V{{n}} in known_prior_templates \
             before updating this pin"
        );
    }
}
