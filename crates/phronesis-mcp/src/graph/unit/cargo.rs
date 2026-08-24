//! `Cargo.toml` parsing: the package name and dependency aliases, plus the
//! small TOML line scanner that `pyproject.toml` parsing shares.

use super::Manifest;
use regex::Regex;
use std::sync::LazyLock;

/// `package = "x"` inside an inline table, anchored so that keys merely
/// *ending* in `package` (`default-package`) do not match.
static PACKAGE_KEY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?:^|[{,\s])package\s*=\s*"([^"]+)""#).expect("static regex compiles")
});

/// Drop a trailing `#` comment, respecting quoted strings so a `#` inside a
/// path or version string is not mistaken for one.
pub(super) fn strip_comment(line: &str) -> &str {
    let mut in_quotes = false;
    for (i, c) in line.char_indices() {
        match c {
            '"' => in_quotes = !in_quotes,
            '#' if !in_quotes => return &line[..i],
            _ => {}
        }
    }
    line
}

/// Net brace depth contributed by a line, ignoring braces inside strings.
fn depth_delta(s: &str) -> i32 {
    let mut in_quotes = false;
    let mut depth = 0;
    for c in s.chars() {
        match c {
            '"' => in_quotes = !in_quotes,
            '{' if !in_quotes => depth += 1,
            '}' if !in_quotes => depth -= 1,
            _ => {}
        }
    }
    depth
}

pub(super) fn unquote(s: &str) -> &str {
    s.trim().trim_matches('"')
}

/// Which alias map a section header feeds, if any.
enum DepTable {
    /// `[dependencies]`, `[dev-dependencies]`, `[target.'…'.dependencies]`
    Local,
    /// `[workspace.dependencies]`
    Workspace,
}

fn dep_table(section: &str) -> Option<DepTable> {
    if section == "workspace.dependencies" || section.starts_with("workspace.dependencies.") {
        return Some(DepTable::Workspace);
    }
    let is_local = section == "dependencies"
        || section == "dev-dependencies"
        || section == "build-dependencies"
        || section.ends_with(".dependencies")
        || section.starts_with("dependencies.")
        || section.starts_with("dev-dependencies.")
        || section.starts_with("build-dependencies.");
    is_local.then_some(DepTable::Local)
}

/// Parse the subset of `Cargo.toml` that bears on identity: the package name
/// and the dependency aliases.
///
/// Hand-written rather than pulling in a TOML parser, because the subset is
/// small and stable: a section header, `name = "…"`, and `package = "…"`
/// inside a dependency's inline table. Anything it fails to understand
/// degrades to "no alias", which loses an edge rather than inventing one.
pub fn parse_cargo_manifest(text: &str) -> Manifest {
    let mut scan = CargoScan::default();
    for raw in text.lines() {
        scan.line(strip_comment(raw).trim());
    }
    scan.out
}

/// Line-by-line state for `parse_cargo_manifest`.
#[derive(Default)]
struct CargoScan {
    out: Manifest,
    /// The `[section]` the current line sits under.
    section: String,
    /// An inline table can span lines; accumulate until the braces balance.
    pending: Option<(String, String, i32)>,
}

impl CargoScan {
    /// Consume one comment-stripped, trimmed line.
    fn line(&mut self, line: &str) {
        if let Some((_, body, depth)) = self.pending.as_mut() {
            body.push(' ');
            body.push_str(line);
            *depth += depth_delta(line);
            if *depth <= 0 {
                let (alias, body, _) = self.pending.take().unwrap_or_default();
                record_dep(&mut self.out, &self.section, alias, &body);
            }
            return;
        }

        if line.starts_with('[') {
            self.section = line.trim_matches(['[', ']']).trim().to_string();
            // `[dependencies.foo]` names its alias in the header; a bare
            // restatement with no `package` key still needs recording.
            if let Some(alias) = self
                .section
                .rsplit_once('.')
                .filter(|(head, _)| dep_table(head).is_some() || head.ends_with("dependencies"))
                .map(|(_, alias)| alias.to_string())
            {
                record_dep(&mut self.out, &self.section, alias, "");
            }
            return;
        }

        let Some((key, value)) = line.split_once('=') else {
            return;
        };
        let (key, value) = (unquote(key), value.trim());

        if self.section == "package" && key == "name" {
            self.out.package = Some(unquote(value).to_string());
            return;
        }
        if dep_table(&self.section).is_none() {
            return;
        }
        // A sub-table header already registered this alias; its body lines
        // only matter for a `package` key, handled by record_dep below.
        if self.section.contains('.')
            && dep_table(&self.section).is_some()
            && !self.section.ends_with("dependencies")
        {
            if key == "package" {
                record_dep_named(&mut self.out, &self.section, unquote(value).to_string());
            }
            return;
        }

        let depth = depth_delta(value);
        if depth > 0 {
            self.pending = Some((key.to_string(), value.to_string(), depth));
        } else {
            record_dep(&mut self.out, &self.section, key.to_string(), value);
        }
    }
}

/// Register `alias`, resolving its real package name from `body` if the inline
/// table renames it.
fn record_dep(out: &mut Manifest, section: &str, alias: String, body: &str) {
    if alias.is_empty() {
        return;
    }
    let package = PACKAGE_KEY
        .captures(body)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string())
        .unwrap_or_else(|| alias.clone());
    let target = match dep_table(section) {
        Some(DepTable::Workspace) => &mut out.workspace_deps,
        Some(DepTable::Local) => &mut out.deps,
        None => return,
    };
    target.insert(alias, package);
}

/// Point an already-registered `[dependencies.<alias>]` sub-table at the real
/// package named by its `package` key.
fn record_dep_named(out: &mut Manifest, section: &str, package: String) {
    let Some((_, alias)) = section.rsplit_once('.') else {
        return;
    };
    let target = match dep_table(section) {
        Some(DepTable::Workspace) => &mut out.workspace_deps,
        Some(DepTable::Local) => &mut out.deps,
        None => return,
    };
    target.insert(alias.to_string(), package);
}
