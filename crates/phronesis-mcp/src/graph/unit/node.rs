//! `package.json` and `tsconfig.json` parsing, the JSONC stripper that makes
//! the latter readable, and the per-unit TypeScript file index.

use super::{LANG_TYPESCRIPT, Manifest, TsConfig, lang_of_path};
use std::path::Path;

/// Parse the one field of `package.json` that bears on identity: `name`.
///
/// Real JSON, not the hand-rolled scanner used for TOML: `package.json` is
/// JSON by definition and `serde_json` is already a dependency, so there is
/// no reason to approximate it. A file that does not parse declares no
/// package, which loses a unit rather than inventing one.
pub fn parse_package_json(text: &str) -> Manifest {
    let mut out = Manifest::default();
    if let Ok(serde_json::Value::Object(map)) = serde_json::from_str(text)
        && let Some(serde_json::Value::String(name)) = map.get("name")
        && !name.is_empty()
    {
        out.package = Some(name.clone());
    }
    out
}

/// Strip `//` line comments, `/* … */` block comments, and trailing commas
/// so `serde_json` can read a `tsconfig.json`.
///
/// TypeScript accepts JSONC here and real projects use it — `tsc --init`'s
/// own generated file opens with a `/* … */` banner — so refusing comments
/// would silently lose resolution rules on the most ordinary files.
/// Both comment forms, and the trailing-comma pass, track string state
/// (with backslash escapes) so nothing inside a string literal is mistaken
/// for comment or container syntax.
fn strip_jsonc(text: &str) -> String {
    strip_trailing_commas(&strip_comments(text))
}

/// Where the comment-stripping pass stands in the input.
#[derive(PartialEq)]
enum CommentState {
    Normal,
    InString,
    InLineComment,
    InBlockComment,
}

/// First pass of `strip_jsonc`: drop `//` and `/* … */` comments.
struct CommentStripper {
    out: String,
    state: CommentState,
    escaped: bool,
}

impl CommentStripper {
    fn feed(&mut self, c: char, chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
        match self.state {
            CommentState::InString => {
                self.out.push(c);
                if self.escaped {
                    self.escaped = false;
                } else if c == '\\' {
                    self.escaped = true;
                } else if c == '"' {
                    self.state = CommentState::Normal;
                }
            }
            CommentState::InLineComment => {
                if c == '\n' {
                    self.out.push('\n');
                    self.state = CommentState::Normal;
                }
                // Everything else inside a line comment, including `/*` or
                // `"`, is just text — it does not open a nested state.
            }
            CommentState::InBlockComment => {
                if c == '*' && chars.peek() == Some(&'/') {
                    chars.next();
                    self.state = CommentState::Normal;
                }
                // A `//` or `"` inside a block comment is likewise inert;
                // only `*/` ends the comment.
            }
            CommentState::Normal => match c {
                '"' => {
                    self.state = CommentState::InString;
                    self.out.push(c);
                }
                '/' if chars.peek() == Some(&'/') => {
                    chars.next();
                    self.state = CommentState::InLineComment;
                }
                '/' if chars.peek() == Some(&'*') => {
                    chars.next();
                    self.state = CommentState::InBlockComment;
                }
                _ => self.out.push(c),
            },
        }
    }
}

fn strip_comments(text: &str) -> String {
    let mut stripper = CommentStripper {
        out: String::with_capacity(text.len()),
        state: CommentState::Normal,
        escaped: false,
    };
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        stripper.feed(c, &mut chars);
    }
    stripper.out
}

/// Second pass of `strip_jsonc`: drop a comma whose next non-whitespace
/// character closes a container. String-aware (with escapes) so a `,}`
/// inside a string value is never mistaken for one.
struct CommaStripper {
    cleaned: String,
    in_string: bool,
    escaped: bool,
}

impl CommaStripper {
    /// Consume `c`; `rest` is everything after it.
    fn feed(&mut self, c: char, rest: &[char]) {
        if self.in_string {
            self.cleaned.push(c);
            if self.escaped {
                self.escaped = false;
            } else if c == '\\' {
                self.escaped = true;
            } else if c == '"' {
                self.in_string = false;
            }
            return;
        }
        if c == '"' {
            self.in_string = true;
            self.cleaned.push(c);
            return;
        }
        if c == ',' {
            let next = rest.iter().find(|n| !n.is_whitespace());
            if matches!(next, Some('}') | Some(']')) {
                return;
            }
        }
        self.cleaned.push(c);
    }
}

fn strip_trailing_commas(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut stripper = CommaStripper {
        cleaned: String::with_capacity(text.len()),
        in_string: false,
        escaped: false,
    };
    for (i, &c) in chars.iter().enumerate() {
        stripper.feed(c, &chars[i + 1..]);
    }
    stripper.cleaned
}

/// Parse the resolution-relevant subset of a `tsconfig.json`.
///
/// `extends` is deliberately not followed here — see Task 4, which resolves
/// the chain on disk where the referenced files are readable.
pub fn parse_tsconfig(text: &str) -> TsConfig {
    let mut out = TsConfig::default();
    let Ok(serde_json::Value::Object(root)) = serde_json::from_str(&strip_jsonc(text)) else {
        return out;
    };
    let Some(serde_json::Value::Object(options)) = root.get("compilerOptions") else {
        return out;
    };
    if let Some(serde_json::Value::String(base)) = options.get("baseUrl") {
        out.base_url = base
            .trim_start_matches("./")
            .trim_end_matches('/')
            .trim_matches('.')
            .trim_matches('/')
            .to_string();
    }
    if let Some(serde_json::Value::Object(paths)) = options.get("paths") {
        for (alias, targets) in paths {
            let Some(serde_json::Value::Array(list)) = Some(targets) else {
                continue;
            };
            let targets: Vec<String> = list
                .iter()
                .filter_map(|t| t.as_str().map(|s| s.trim_start_matches("./").to_string()))
                .collect();
            if !targets.is_empty() {
                out.paths.insert(alias.clone(), targets);
            }
        }
    }
    out
}

/// Depth cap for `extends`. A chain longer than this is a configuration
/// mistake or a cycle; stopping is better than recursing forever.
const MAX_EXTENDS_DEPTH: usize = 8;

/// Read a `tsconfig.json` and everything it extends, child winning.
///
/// Only `extends` targets that are relative paths are followed. A bare
/// specifier resolves inside `node_modules`, which this graph never reads.
pub(super) fn read_tsconfig_chain(path: &Path, depth: usize) -> TsConfig {
    if depth >= MAX_EXTENDS_DEPTH {
        return TsConfig::default();
    }
    let Ok(text) = std::fs::read_to_string(path) else {
        return TsConfig::default();
    };
    let own = parse_tsconfig(&text);

    let parent = serde_json::from_str::<serde_json::Value>(&strip_jsonc(&text))
        .ok()
        .and_then(|v| v.get("extends")?.as_str().map(str::to_string))
        .filter(|e| e.starts_with('.'))
        .and_then(|e| {
            let mut candidate = path.parent()?.join(&e);
            if candidate.extension().is_none() {
                candidate.set_extension("json");
            }
            Some(read_tsconfig_chain(&candidate, depth + 1))
        });

    let Some(mut merged) = parent else {
        return own;
    };
    if !own.base_url.is_empty() {
        merged.base_url = own.base_url;
    }
    merged.paths.extend(own.paths);
    merged
}

/// Repo-relative TypeScript sources under `unit_abs`, honouring `.gitignore`,
/// excluding `node_modules` unconditionally, and stopping at the boundary of
/// any nested unit.
///
/// A nested `package.json` starts a unit of its own; descending past it
/// would let the outer unit's file index claim files that actually belong to
/// the inner one. `resolve` is membership-based against this index (see its
/// doc comment), so a polluted index does not just misname a file — it can
/// resolve an import in the outer unit to a file that identifies itself
/// under the inner unit's `id`/`module_base`, producing an edge that never
/// joins the file's own `declares_module`. Better to under-index (parent
/// misses a file it never owned) than to over-index (parent claims a file it
/// doesn't own).
pub(super) fn index_typescript_files(root: &Path, unit_abs: &Path, unit_rel: &str) -> Vec<String> {
    let mut out = Vec::new();
    let unit_abs_owned = unit_abs.to_path_buf();
    for entry in ignore::WalkBuilder::new(unit_abs)
        .hidden(true)
        .filter_entry(move |e| {
            if e.file_name() == "node_modules" {
                return false;
            }
            // The unit's own root always carries a package.json; only a
            // *nested* directory carrying one marks a boundary to stop at.
            e.path() == unit_abs_owned
                || !e
                    .file_type()
                    .is_some_and(|t| t.is_dir() && e.path().join("package.json").is_file())
        })
        .build()
        .flatten()
    {
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        if lang_of_path(entry.path().to_str().unwrap_or("")) != Some(LANG_TYPESCRIPT) {
            continue;
        }
        if let Ok(rel) = entry.path().strip_prefix(root)
            && let Some(rel) = rel.to_str()
        {
            out.push(rel.replace('\\', "/"));
        }
    }
    let _ = unit_rel;
    out.sort();
    out
}
