//! `Package.swift` parsing: one manifest declares many targets, each its own
//! module namespace.

use regex::Regex;
use std::sync::LazyLock;

/// One target declared by a `Package.swift`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwiftTarget {
    /// `name:` as declared — also the module name source imports.
    pub name: String,
    /// Directory holding the target's sources, relative to the manifest.
    pub path: String,
    /// Declared with `.testTarget`.
    pub is_test: bool,
}

/// `.target(name: "X"`, `.executableTarget(name: "X"`, `.testTarget(name: "X"`
/// (plus `.macro`, which compiles like a target). Products, plugins, binary
/// and system-library targets hold no Swift sources this graph would index.
static SWIFT_TARGET: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"\.(target|executableTarget|testTarget|macro)\s*\(\s*name\s*:\s*"([^"]+)""#)
        .expect("static regex compiles")
});

/// `path: "…"` inside a target's argument list.
static SWIFT_TARGET_PATH: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"[\s(,]path\s*:\s*"([^"]+)""#).expect("static regex compiles"));

/// Parse the target list of a `Package.swift`.
///
/// Regex over the manifest text, not Swift evaluation — the same trade
/// `cue.rs` makes for `module.cue`. A target's arguments are taken to run
/// until the next target declaration, which is where an explicit `path:`
/// is looked for; SwiftPM's defaults (`Sources/<Name>` and
/// `Tests/<Name>`) apply otherwise. A manifest that builds its target list
/// programmatically yields nothing, which leaves every file on the
/// `swift:project` fallback rather than naming it under a guessed target.
///
/// Xcode `.xcodeproj` projects have no manifest this can read (target
/// membership lives in `project.pbxproj`, which is not parsed); their files
/// stay on the fallback and the extractor's filename/directory heuristic
/// decides `file_type`.
pub fn parse_package_swift(text: &str) -> Vec<SwiftTarget> {
    let heads: Vec<_> = SWIFT_TARGET.captures_iter(text).collect();
    heads
        .iter()
        .enumerate()
        .filter_map(|(i, cap)| {
            let kind = cap.get(1)?.as_str();
            let name = cap.get(2)?.as_str().to_string();
            let start = cap.get(0)?.end();
            let end = heads
                .get(i + 1)
                .and_then(|next| next.get(0))
                .map_or(text.len(), |m| m.start());
            let is_test = kind == "testTarget";
            let path = SWIFT_TARGET_PATH
                .captures(&text[start..end])
                .and_then(|c| c.get(1))
                .map(|m| {
                    m.as_str()
                        .trim_start_matches("./")
                        .trim_matches('/')
                        .to_string()
                })
                .unwrap_or_else(|| format!("{}/{name}", if is_test { "Tests" } else { "Sources" }));
            Some(SwiftTarget {
                name,
                path,
                is_test,
            })
        })
        .collect()
}
