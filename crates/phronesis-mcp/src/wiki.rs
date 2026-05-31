//! Wiki page primitives — ADR-style decision pages under
//! `.phronesis/wiki/decisions/`. Shared by `wiki_drift` and by any
//! future wiki-consuming module.

use std::path::{Path, PathBuf};

use serde::Deserialize;
use thiserror::Error;

/// Structured YAML frontmatter at the top of every decision page.
/// See SPEC-wiki-drift.md §"Decision page schema".
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionFrontmatter {
    pub id: String,
    pub date: String,
    pub status: String,
    #[serde(default)]
    pub enforces: Vec<String>,
    #[serde(default)]
    pub superseded_by: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
}

/// One decision page on disk: structured frontmatter + free-form body.
#[derive(Debug, Clone)]
pub struct Decision {
    pub frontmatter: DecisionFrontmatter,
    pub body: String,
    pub path: PathBuf,
}

#[derive(Debug, Error)]
pub enum WikiError {
    #[error("wiki directory not found at {0}")]
    DirMissing(String),
    #[error("failed to read {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("{path}: missing or malformed frontmatter ({message})")]
    Frontmatter { path: String, message: String },
}

/// Default per-project wiki directory: `<project_root>/.phronesis/wiki/`.
pub fn default_wiki_dir(project_root: &Path) -> PathBuf {
    project_root.join(".phronesis").join("wiki")
}

/// Parse a single decision page. Expects:
///
/// ```text
/// ---
/// id: ...
/// date: ...
/// ---
///
/// body text
/// ```
pub fn parse_decision_file(path: &Path) -> Result<Decision, WikiError> {
    let content = std::fs::read_to_string(path).map_err(|source| WikiError::Io {
        path: path.display().to_string(),
        source,
    })?;

    // Frontmatter is bracketed by `---` on its own line at the top of the file.
    // We accept an optional leading whitespace before the opening fence.
    let trimmed = content.trim_start_matches('\u{FEFF}'); // strip UTF-8 BOM if present
    let rest = trimmed
        .strip_prefix("---\n")
        .or_else(|| trimmed.strip_prefix("---\r\n"))
        .ok_or_else(|| WikiError::Frontmatter {
            path: path.display().to_string(),
            message: "expected file to start with `---`".to_string(),
        })?;
    let close_idx = rest.find("\n---").ok_or_else(|| WikiError::Frontmatter {
        path: path.display().to_string(),
        message: "missing closing `---` fence".to_string(),
    })?;
    let yaml = &rest[..close_idx];
    // Body starts after the closing fence and the newline that follows it.
    let after_fence = &rest[close_idx + 4..]; // skip "\n---"
    let body = after_fence
        .strip_prefix('\n')
        .or_else(|| after_fence.strip_prefix("\r\n"))
        .unwrap_or(after_fence)
        .to_string();

    let frontmatter: DecisionFrontmatter =
        serde_yml::from_str(yaml).map_err(|e| WikiError::Frontmatter {
            path: path.display().to_string(),
            message: e.to_string(),
        })?;

    Ok(Decision {
        frontmatter,
        body,
        path: path.to_path_buf(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write(dir: &TempDir, name: &str, content: &str) -> PathBuf {
        let p = dir.path().join(name);
        fs::write(&p, content).expect("write fixture");
        p
    }

    #[test]
    fn parse_minimal_decision() {
        let dir = TempDir::new().unwrap();
        let p = write(
            &dir,
            "2026-05-29-card-game-vocab.md",
            "---\n\
             id: card-game-vocab\n\
             date: 2026-05-29\n\
             status: accepted\n\
             ---\n\
             \n\
             ## Decision\n\
             Use card-game vocabulary.\n",
        );
        let d = parse_decision_file(&p).expect("parse");
        assert_eq!(d.frontmatter.id, "card-game-vocab");
        assert_eq!(d.frontmatter.date, "2026-05-29");
        assert_eq!(d.frontmatter.status, "accepted");
        assert!(d.frontmatter.enforces.is_empty());
        assert!(d.frontmatter.superseded_by.is_none());
        assert!(d.body.contains("## Decision"));
        assert!(d.body.contains("Use card-game vocabulary"));
        assert_eq!(d.path, p);
    }

    #[test]
    fn parse_decision_with_enforces_list_and_tags() {
        let dir = TempDir::new().unwrap();
        let p = write(
            &dir,
            "x.md",
            "---\n\
             id: workspace-cargo\n\
             date: 2026-05-29\n\
             status: accepted\n\
             enforces:\n  - warn-cargo-build-without-workspace\n  - some-other-rule\n\
             tags: [build, hygiene]\n\
             ---\n\
             body\n",
        );
        let d = parse_decision_file(&p).expect("parse");
        assert_eq!(
            d.frontmatter.enforces,
            vec![
                "warn-cargo-build-without-workspace".to_string(),
                "some-other-rule".to_string()
            ]
        );
        assert_eq!(
            d.frontmatter.tags,
            vec!["build".to_string(), "hygiene".to_string()]
        );
    }

    #[test]
    fn parse_decision_with_superseded_by() {
        let dir = TempDir::new().unwrap();
        let p = write(
            &dir,
            "x.md",
            "---\n\
             id: old\n\
             date: 2026-01-01\n\
             status: superseded\n\
             superseded_by: new-decision\n\
             ---\n\
             ",
        );
        let d = parse_decision_file(&p).expect("parse");
        assert_eq!(
            d.frontmatter.superseded_by,
            Some("new-decision".to_string())
        );
    }

    #[test]
    fn parse_decision_missing_frontmatter_errors() {
        let dir = TempDir::new().unwrap();
        let p = write(&dir, "x.md", "no frontmatter here\njust prose\n");
        match parse_decision_file(&p) {
            Err(WikiError::Frontmatter { .. }) => {}
            other => panic!("expected Frontmatter error, got {:?}", other),
        }
    }

    #[test]
    fn parse_decision_missing_required_field_errors() {
        let dir = TempDir::new().unwrap();
        let p = write(
            &dir,
            "x.md",
            "---\nid: x\ndate: 2026-01-01\n---\nbody\n", // missing `status`
        );
        match parse_decision_file(&p) {
            Err(WikiError::Frontmatter { message, .. }) => assert!(message.contains("status")),
            other => panic!("expected Frontmatter error, got {:?}", other),
        }
    }

    #[test]
    fn parse_decision_unknown_field_errors() {
        // deny_unknown_fields catches typos that would otherwise silently drop.
        let dir = TempDir::new().unwrap();
        let p = write(
            &dir,
            "x.md",
            "---\nid: x\ndate: 2026-01-01\nstatus: accepted\nstauts: typo\n---\n",
        );
        assert!(matches!(
            parse_decision_file(&p),
            Err(WikiError::Frontmatter { .. })
        ));
    }
}
