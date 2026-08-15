//! Explicit generated-data contracts from `.phronesis/graph.toml`.

use super::model::Edge;
use regex::Regex;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::LazyLock;

#[derive(Debug, Default)]
struct Binding {
    producer: String,
    artifact: String,
    consumer: Option<String>,
}

#[derive(Debug, Default)]
struct SerdeType {
    fields: BTreeMap<String, BTreeSet<String>>,
    flatten: bool,
    deny_unknown_fields: bool,
}

static STRUCT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?s)(?P<attrs>(?:#\s*\[[^\]]*\]\s*)*)(?:pub(?:\([^)]*\))?\s+)?struct\s+(?P<name>[A-Za-z_][A-Za-z0-9_]*)\s*(?:<[^>]*>)?\s*\{(?P<body>.*?)\}")
        .expect("static regex")
});
static FIELD_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)(?P<attrs>(?:\s*#\s*\[[^\]]*\]\s*)*)\s*(?:pub(?:\([^)]*\))?\s+)?(?P<name>[A-Za-z_][A-Za-z0-9_]*)\s*:")
        .expect("static regex")
});
static STRING_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"\"([^\"]+)\""#).expect("static regex"));
static CONCAT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"concat!\s*\((?P<parts>(?:\s*\"[^\"]*\"\s*,?)+)\)"#).expect("static regex")
});
static FROM_TYPE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:from_(?:str|slice|reader|value))\s*::\s*<\s*([A-Za-z_][A-Za-z0-9_:]*)")
        .expect("static regex")
});
static LET_TYPE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"let\s+[A-Za-z_][A-Za-z0-9_]*\s*:\s*([A-Za-z_][A-Za-z0-9_:]*)\s*=")
        .expect("static regex")
});

fn quoted_value(line: &str) -> Option<String> {
    let (_, value) = line.split_once('=')?;
    let value = value.trim();
    let quote = value
        .chars()
        .next()
        .filter(|quote| matches!(quote, '"' | '\''))?;
    let rest = value.strip_prefix(quote)?;
    let end = rest.find(quote)?;
    let trailing = rest[end + quote.len_utf8()..].trim();
    if !trailing.is_empty() && !trailing.starts_with('#') {
        return None;
    }
    Some(rest[..end].to_string())
}

fn load_bindings(root: &Path) -> Vec<Binding> {
    let Ok(content) = std::fs::read_to_string(root.join(".phronesis/graph.toml")) else {
        return Vec::new();
    };
    let mut bindings = Vec::new();
    let mut current: Option<Binding> = None;
    for raw in content.lines() {
        let line = raw.trim();
        if line.starts_with('#') {
            continue;
        }
        // Section-aware: any header ends the binding in progress. Without
        // this, every `key = value` line after a `[[generated_artifacts]]`
        // block is absorbed into that block no matter what table intervenes,
        // so adding an unrelated section — `[ownership.rust]`, say — to a file
        // that already has bindings silently rewrites them.
        if line.starts_with('[') {
            if line != "[[generated_artifacts]]" {
                if let Some(binding) = current.take() {
                    bindings.push(binding);
                }
                continue;
            }
            if let Some(binding) = current.take() {
                bindings.push(binding);
            }
            current = Some(Binding::default());
        } else if let Some(binding) = current.as_mut()
            && let Some((key, _)) = line.split_once('=')
        {
            match key.trim() {
                "producer" => binding.producer = quoted_value(line).unwrap_or_default(),
                "artifact" => binding.artifact = quoted_value(line).unwrap_or_default(),
                "consumer" => binding.consumer = quoted_value(line),
                _ => {}
            }
        }
    }
    if let Some(binding) = current {
        bindings.push(binding);
    }
    bindings
}

/// Whether `.phronesis/graph.toml` explicitly names `file_path` as a
/// generated artifact.
///
/// The save pipeline uses this to decide whether an edit invalidates the
/// data-contract edges, which are attributed to `graph.toml` rather than to
/// the artifact and so survive provenance-keyed compaction untouched. Asking
/// the bindings is what keeps that decision from degenerating into "the config
/// file exists, so rebuild everything" (decision D17).
pub(crate) fn declares_artifact(root: &Path, file_path: &str) -> bool {
    load_bindings(root)
        .iter()
        .any(|binding| binding.artifact == file_path)
}

fn serde_option(attrs: &str, name: &str) -> Option<String> {
    let needle = format!("{name} = \"");
    let start = attrs.find(&needle)? + needle.len();
    let end = attrs[start..].find('"')?;
    Some(attrs[start..start + end].to_string())
}

fn rename(wire: &str, rule: Option<&str>) -> String {
    let words: Vec<String> = wire
        .split('_')
        .filter(|word| !word.is_empty())
        .map(str::to_ascii_lowercase)
        .collect();
    match rule {
        Some("camelCase") if !words.is_empty() => {
            let mut out = words[0].clone();
            for word in &words[1..] {
                let mut chars = word.chars();
                if let Some(first) = chars.next() {
                    out.push(first.to_ascii_uppercase());
                    out.extend(chars);
                }
            }
            out
        }
        Some("PascalCase") => words
            .iter()
            .map(|word| {
                let mut chars = word.chars();
                chars
                    .next()
                    .map(|first| first.to_ascii_uppercase().to_string() + chars.as_str())
                    .unwrap_or_default()
            })
            .collect(),
        Some("kebab-case") => words.join("-"),
        Some("SCREAMING_SNAKE_CASE") => words.join("_").to_ascii_uppercase(),
        Some("snake_case") | None => words.join("_"),
        Some(_) => wire.to_string(),
    }
}

fn extract_serde_types(root: &Path, base: &[Edge]) -> (Vec<Edge>, BTreeMap<String, SerdeType>) {
    let modules: BTreeMap<&str, &str> = base
        .iter()
        .filter(|edge| {
            edge.p == "declares_module" && edge.a.len() == 2 && edge.a[1].starts_with("rust:")
        })
        .map(|edge| (edge.a[0].as_str(), edge.a[1].as_str()))
        .collect();
    let mut edges = Vec::new();
    let mut types = BTreeMap::new();
    for (file, module) in modules {
        let Ok(content) = std::fs::read_to_string(root.join(file)) else {
            continue;
        };
        for capture in STRUCT_RE.captures_iter(&content) {
            let attrs = capture.name("attrs").map_or("", |value| value.as_str());
            if !attrs.contains("Deserialize") {
                continue;
            }
            let Some(name) = capture.name("name").map(|value| value.as_str()) else {
                continue;
            };
            let type_id = format!("{module}::{name}");
            let mut serde_type = SerdeType {
                deny_unknown_fields: attrs.contains("deny_unknown_fields"),
                ..SerdeType::default()
            };
            let rename_all = serde_option(attrs, "rename_all");
            let body = capture.name("body").map_or("", |value| value.as_str());
            for field in FIELD_RE.captures_iter(body) {
                let field_attrs = field.name("attrs").map_or("", |value| value.as_str());
                let Some(field_name) = field.name("name").map(|value| value.as_str()) else {
                    continue;
                };
                if field_attrs.contains("flatten") {
                    serde_type.flatten = true;
                }
                let field_id = format!("{type_id}::{field_name}");
                let mut accepted = BTreeSet::new();
                accepted.insert(
                    serde_option(field_attrs, "rename")
                        .unwrap_or_else(|| rename(field_name, rename_all.as_deref())),
                );
                let mut rest = field_attrs;
                while let Some(position) = rest.find("alias = \"") {
                    rest = &rest[position + 9..];
                    if let Some(end) = rest.find('"') {
                        accepted.insert(rest[..end].to_string());
                        rest = &rest[end + 1..];
                    } else {
                        break;
                    }
                }
                for wire_name in &accepted {
                    edges.push(Edge::base(
                        "serde_field",
                        &[&type_id, &field_id, wire_name],
                        file,
                    ));
                }
                serde_type.fields.insert(field_id, accepted);
            }
            edges.push(Edge::base("graph_definition", &[&type_id], file));
            edges.push(Edge::base("defines", &[file, &type_id], file));
            edges.push(Edge::base("element_in_file", &[&type_id, file], file));
            edges.push(Edge::base("element_in_module", &[&type_id, module], file));
            types.insert(type_id, serde_type);
        }
    }
    (edges, types)
}

fn pointer(key: &str) -> String {
    format!("/{}", key.replace('~', "~0").replace('/', "~1"))
}

fn function_chunks(content: &str) -> impl Iterator<Item = &str> {
    let mut starts = content
        .match_indices("fn ")
        .map(|(position, _)| position)
        .collect::<Vec<_>>();
    starts.push(content.len());
    starts
        .windows(2)
        .map(|window| &content[window[0]..window[1]])
        .collect::<Vec<_>>()
        .into_iter()
}

fn function_name(chunk: &str) -> Option<&str> {
    let rest = chunk.strip_prefix("fn ")?;
    rest.split(|character: char| !(character.is_ascii_alphanumeric() || character == '_'))
        .next()
        .filter(|name| !name.is_empty())
}

fn literal_paths(chunk: &str, suffixes: &[&str]) -> BTreeSet<String> {
    let is_path = |value: &str| {
        !value.chars().any(char::is_whitespace)
            && !value.contains(['{', '}'])
            && !value.starts_with("cargo:")
            && !value.starts_with('=')
            && suffixes.iter().any(|suffix| value.ends_with(suffix))
    };
    let mut paths = BTreeSet::new();
    let mut outside_concat = chunk.to_string();
    for capture in CONCAT_RE.captures_iter(chunk) {
        let Some(whole) = capture.get(0) else {
            continue;
        };
        let value = capture
            .name("parts")
            .into_iter()
            .flat_map(|parts| STRING_RE.captures_iter(parts.as_str()))
            .filter_map(|part| part.get(1).map(|value| value.as_str()))
            .collect::<String>();
        if is_path(&value) {
            paths.insert(value.trim_start_matches("./").to_string());
        }
        outside_concat.replace_range(whole.range(), &" ".repeat(whole.as_str().len()));
    }
    paths.extend(
        STRING_RE
            .captures_iter(&outside_concat)
            .filter_map(|capture| capture.get(1).map(|value| value.as_str()))
            .filter(|value| is_path(value))
            .map(|value| value.trim_start_matches("./").to_string()),
    );
    paths
}

fn diagnostic(additions: &mut BTreeSet<(String, Vec<String>)>, kind: &str, reference: &str) {
    additions.insert((
        "generated_artifact_diagnostic".to_string(),
        vec![kind.to_string(), reference.to_string()],
    ));
}

fn infer_bindings(
    root: &Path,
    base: &[Edge],
    serde_types: &BTreeMap<String, SerdeType>,
) -> Vec<Binding> {
    let cue_modules: BTreeMap<&str, BTreeSet<&str>> = base
        .iter()
        .filter(|edge| {
            edge.p == "declares_module" && edge.a.len() == 2 && edge.a[0].ends_with(".cue")
        })
        .fold(BTreeMap::new(), |mut map, edge| {
            map.entry(edge.a[0].as_str())
                .or_default()
                .insert(edge.a[1].as_str());
            map
        });
    let mut inferred: BTreeMap<String, Binding> = BTreeMap::new();
    for entry in ignore::WalkBuilder::new(root)
        .hidden(true)
        .build()
        .flatten()
    {
        let path = entry.path();
        if !entry.file_type().is_some_and(|kind| kind.is_file())
            || path.extension().and_then(|value| value.to_str()) != Some("rs")
        {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(path) else {
            continue;
        };
        for chunk in function_chunks(&content) {
            if chunk.contains("Command::new(\"cue\")")
                && chunk.contains("\"export\"")
                && chunk.contains("stdout")
                && (chunk.contains("yaml") || chunk.contains("json"))
            {
                let cue_paths = literal_paths(chunk, &[".cue"]);
                let artifacts = literal_paths(chunk, &[".yaml", ".yml", ".json"]);
                if cue_paths.len() == 1 && artifacts.len() == 1 {
                    let cue_path = cue_paths.iter().next().expect("one cue path");
                    let artifact = artifacts.iter().next().expect("one artifact");
                    if root.join(cue_path).is_file()
                        && root.join(artifact).is_file()
                        && let Some(modules) = cue_modules.get(cue_path.as_str())
                        && modules.len() == 1
                    {
                        let binding = inferred.entry(artifact.clone()).or_insert_with(|| Binding {
                            artifact: artifact.clone(),
                            ..Binding::default()
                        });
                        binding.producer =
                            (*modules.iter().next().expect("one module")).to_string();
                    }
                }
            }
            if (chunk.contains("serde_yaml::") || chunk.contains("serde_json::"))
                && (chunk.contains("from_str")
                    || chunk.contains("from_slice")
                    || chunk.contains("from_reader")
                    || chunk.contains("from_value"))
            {
                let artifacts = literal_paths(chunk, &[".yaml", ".yml", ".json"]);
                let target = FROM_TYPE_RE
                    .captures(chunk)
                    .or_else(|| LET_TYPE_RE.captures(chunk))
                    .and_then(|capture| capture.get(1).map(|value| value.as_str()));
                if artifacts.len() == 1 {
                    let typed_consumer = target.and_then(|target| {
                        let simple = target.rsplit("::").next().unwrap_or(target);
                        let matches = serde_types
                            .keys()
                            .filter(|type_id| type_id.ends_with(&format!("::{simple}")))
                            .cloned()
                            .collect::<Vec<_>>();
                        (matches.len() == 1).then(|| matches[0].clone())
                    });
                    let function_consumer = function_name(chunk).and_then(|name| {
                        let source = path.strip_prefix(root).ok()?.to_str()?.replace('\\', "/");
                        let matches = base
                            .iter()
                            .filter(|edge| {
                                edge.p == "defines_fn"
                                    && edge.src == source
                                    && edge.a.get(1).is_some_and(|identity| {
                                        identity.rsplit("::").next() == Some(name)
                                    })
                            })
                            .filter_map(|edge| edge.a.get(1).cloned())
                            .collect::<BTreeSet<_>>();
                        (matches.len() == 1)
                            .then(|| matches.into_iter().next())
                            .flatten()
                    });
                    if let Some(consumer) = typed_consumer.or(function_consumer) {
                        let artifact = artifacts.iter().next().expect("one artifact");
                        inferred
                            .entry(artifact.clone())
                            .or_insert_with(|| Binding {
                                artifact: artifact.clone(),
                                ..Binding::default()
                            })
                            .consumer = Some(consumer);
                    }
                }
            }
        }
    }
    inferred
        .into_values()
        .filter(|binding| !binding.producer.is_empty())
        .collect()
}

fn artifact_keys(root: &Path, path: &str) -> Option<Vec<String>> {
    let content = std::fs::read_to_string(root.join(path)).ok()?;
    if path.ends_with(".json") {
        let value: serde_json::Value = serde_json::from_str(&content).ok()?;
        let object = value.as_object()?;
        Some(object.keys().map(|key| pointer(key)).collect())
    } else {
        if let Ok(value) = serde_norway::from_str::<serde_norway::Value>(&content)
            && let Some(mapping) = value.as_mapping()
        {
            return Some(
                mapping
                    .keys()
                    .filter_map(|key| key.as_str())
                    .map(pointer)
                    .collect(),
            );
        }
        let keys = content
            .lines()
            .filter(|line| !line.starts_with(char::is_whitespace))
            .filter_map(|line| line.split_once(':').map(|(key, _)| key.trim()))
            .filter(|key| {
                !key.is_empty()
                    && !key.starts_with('#')
                    && !key.starts_with('-')
                    && !key.contains(['{', '['])
            })
            .map(pointer)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        (!keys.is_empty()).then_some(keys)
    }
}

pub fn augment(root: &Path, base: &mut Vec<Edge>) {
    let (serde_edges, serde_types) = extract_serde_types(root, base);
    base.extend(serde_edges);
    let graph_ids: BTreeSet<String> = base
        .iter()
        .filter_map(|edge| match edge.p.as_str() {
            "graph_definition" | "graph_function" | "graph_module" => edge.a.first().cloned(),
            "defines_fn" => edge.a.get(1).cloned(),
            "declares_module" => edge.a.get(1).cloned(),
            _ => None,
        })
        .collect();
    let artifact_modules: BTreeMap<String, Vec<String>> = base
        .iter()
        .filter(|edge| edge.p == "declares_module" && edge.a.len() == 2)
        .fold(BTreeMap::new(), |mut map, edge| {
            map.entry(edge.a[0].clone())
                .or_default()
                .push(edge.a[1].clone());
            map
        });
    let mut additions = BTreeSet::new();
    let mut bindings = load_bindings(root);
    let explicit_artifacts = bindings
        .iter()
        .map(|binding| binding.artifact.clone())
        .collect::<BTreeSet<_>>();
    bindings.extend(
        infer_bindings(root, base, &serde_types)
            .into_iter()
            .filter(|binding| !explicit_artifacts.contains(&binding.artifact)),
    );
    for binding in bindings {
        let structured_artifact = binding.artifact.ends_with(".json")
            || binding.artifact.ends_with(".yaml")
            || binding.artifact.ends_with(".yml");
        let modules = artifact_modules
            .get(&binding.artifact)
            .cloned()
            .or_else(|| {
                (structured_artifact && root.join(&binding.artifact).is_file()).then(|| {
                    let language = if binding.artifact.ends_with(".json") {
                        "json"
                    } else {
                        "yaml"
                    };
                    let stem = binding
                        .artifact
                        .trim_end_matches(".json")
                        .trim_end_matches(".yaml")
                        .trim_end_matches(".yml")
                        .replace('/', "::");
                    vec![format!("{language}:project::{stem}")]
                })
            });
        let Some(modules) = modules else {
            tracing::warn!(artifact = %binding.artifact, "graph binding artifact is missing");
            diagnostic(&mut additions, "missing_artifact", &binding.artifact);
            continue;
        };
        if modules.len() != 1 {
            diagnostic(&mut additions, "ambiguous_artifact", &binding.artifact);
            continue;
        }
        if !graph_ids.contains(&binding.producer) {
            tracing::warn!(artifact = %binding.artifact, producer = %binding.producer, "invalid or ambiguous graph binding");
            diagnostic(&mut additions, "missing_producer", &binding.producer);
            continue;
        }
        if binding
            .consumer
            .as_ref()
            .is_some_and(|consumer| !graph_ids.contains(consumer))
        {
            tracing::warn!(consumer = ?binding.consumer, "graph binding consumer is missing");
            diagnostic(
                &mut additions,
                "missing_consumer",
                binding.consumer.as_deref().unwrap_or_default(),
            );
            continue;
        }
        let artifact = &modules[0];
        if !artifact_modules.contains_key(&binding.artifact) {
            additions.insert(("graph_module".to_string(), vec![artifact.clone()]));
            additions.insert((
                "declares_module".to_string(),
                vec![binding.artifact.clone(), artifact.clone()],
            ));
        }
        additions.insert((
            "generates".to_string(),
            vec![binding.producer.clone(), artifact.clone()],
        ));
        let keys = if structured_artifact {
            let Some(keys) = artifact_keys(root, &binding.artifact) else {
                tracing::warn!(artifact = %binding.artifact, "graph binding artifact could not be parsed as a top-level document");
                diagnostic(&mut additions, "malformed_artifact", &binding.artifact);
                continue;
            };
            keys
        } else {
            Vec::new()
        };
        for key in keys {
            additions.insert(("data_key".to_string(), vec![artifact.clone(), key.clone()]));
            additions.insert((
                "emits_key".to_string(),
                vec![binding.producer.clone(), artifact.clone(), key.clone()],
            ));
            if let Some(consumer) = &binding.consumer
                && let Some(serde_type) = serde_types.get(consumer)
            {
                let wire = key
                    .strip_prefix('/')
                    .unwrap_or(&key)
                    .replace("~1", "/")
                    .replace("~0", "~");
                let matching = serde_type
                    .fields
                    .iter()
                    .find(|(_, names)| names.contains(&wire));
                if let Some((field, _)) = matching {
                    additions.insert((
                        "maps_data_key".to_string(),
                        vec![artifact.clone(), key.clone(), field.clone()],
                    ));
                } else if !serde_type.flatten && !serde_type.deny_unknown_fields {
                    additions.insert((
                        "unconsumed_data_key".to_string(),
                        vec![artifact.clone(), key.clone(), consumer.clone()],
                    ));
                }
            }
        }
        if let Some(consumer) = binding.consumer {
            additions.insert((
                "consumes_data".to_string(),
                vec![consumer.clone(), artifact.clone()],
            ));
            if serde_types.contains_key(&consumer) {
                additions.insert(("deserializes".to_string(), vec![consumer, artifact.clone()]));
            }
        }
    }
    base.extend(additions.into_iter().map(|(p, a)| Edge {
        p,
        a,
        src: ".phronesis/graph.toml".to_string(),
        d: false,
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, path: &str, content: &str) {
        let full = root.join(path);
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent).expect("parent");
        }
        std::fs::write(full, content).expect("write fixture");
    }

    fn base() -> Vec<Edge> {
        vec![
            Edge::base(
                "graph_definition",
                &["cue:example.game::cue::export::export::#Manifest"],
                "cue/export/manifest.cue",
            ),
            Edge::base(
                "declares_module",
                &["config/manifest.yaml", "yaml:project::config::manifest"],
                "config/manifest.yaml",
            ),
            Edge::base(
                "declares_module",
                &["src/config.rs", "rust:example-app::config"],
                "src/config.rs",
            ),
        ]
    }

    #[test]
    fn explicit_binding_maps_serde_names_and_reports_silent_drops() {
        let temp = tempfile::tempdir().expect("tempdir");
        write(
            temp.path(),
            "config/manifest.yaml",
            "gameName: demo\nlegacy-name: old\nunmatched: true\na/b: slash\n",
        );
        write(
            temp.path(),
            "src/config.rs",
            r#"#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GameManifest {
    pub game_name: String,
    #[serde(rename = "current", alias = "legacy-name")]
    pub renamed: String,
}
"#,
        );
        write(
            temp.path(),
            ".phronesis/graph.toml",
            r#"[[generated_artifacts]]
producer = "cue:example.game::cue::export::export::#Manifest"
artifact = "config/manifest.yaml"
consumer = "rust:example-app::config::GameManifest"
"#,
        );
        let mut edges = base();
        augment(temp.path(), &mut edges);
        assert!(edges.iter().any(|edge| edge.p == "generates"));
        assert!(
            edges
                .iter()
                .any(|edge| { edge.p == "maps_data_key" && edge.a[1] == "/gameName" })
        );
        assert!(
            edges
                .iter()
                .any(|edge| { edge.p == "maps_data_key" && edge.a[1] == "/legacy-name" })
        );
        assert!(
            edges
                .iter()
                .any(|edge| { edge.p == "data_key" && edge.a[1] == "/a~1b" })
        );
        assert!(
            edges
                .iter()
                .any(|edge| { edge.p == "unconsumed_data_key" && edge.a[1] == "/unmatched" })
        );
    }

    #[test]
    fn flatten_and_deny_unknown_fields_suppress_silent_drop_findings() {
        for serde_attribute in ["flatten", "deny_unknown_fields"] {
            let temp = tempfile::tempdir().expect("tempdir");
            write(temp.path(), "config/manifest.json", "{\"extra\": true}");
            let source = if serde_attribute == "flatten" {
                "#[derive(Deserialize)]\npub struct GameManifest { #[serde(flatten)] pub rest: Map<String, Value>, }"
            } else {
                "#[derive(Deserialize)]\n#[serde(deny_unknown_fields)]\npub struct GameManifest { pub known: String, }"
            };
            write(temp.path(), "src/config.rs", source);
            write(
                temp.path(),
                ".phronesis/graph.toml",
                "[[generated_artifacts]]\nproducer = \"cue:example.game::cue::export::export::#Manifest\"\nartifact = \"config/manifest.json\"\nconsumer = \"rust:example-app::config::GameManifest\"\n",
            );
            let mut edges = base();
            edges.retain(|edge| {
                edge.a
                    .first()
                    .is_none_or(|value| value != "config/manifest.yaml")
            });
            edges.push(Edge::base(
                "declares_module",
                &["config/manifest.json", "json:project::config::manifest"],
                "config/manifest.json",
            ));
            augment(temp.path(), &mut edges);
            assert!(!edges.iter().any(|edge| edge.p == "unconsumed_data_key"));
        }
    }

    #[test]
    fn invalid_exact_references_emit_no_guessed_edges() {
        let temp = tempfile::tempdir().expect("tempdir");
        write(temp.path(), "config/manifest.yaml", "name: demo\n");
        write(
            temp.path(),
            ".phronesis/graph.toml",
            "[[generated_artifacts]]\nproducer = \"cue:missing\"\nartifact = \"config/manifest.yaml\"\n",
        );
        let mut edges = base();
        augment(temp.path(), &mut edges);
        assert!(!edges.iter().any(|edge| edge.p == "generates"));
        assert!(edges.iter().any(|edge| {
            edge.p == "generated_artifact_diagnostic"
                && edge.a == ["missing_producer", "cue:missing"]
        }));
    }

    #[test]
    fn generic_and_private_deserialize_structs_are_indexed() {
        let temp = tempfile::tempdir().expect("tempdir");
        write(
            temp.path(),
            "src/config.rs",
            "#[derive(Deserialize)]\nstruct Private<T> { value: T }\n",
        );
        let (edges, types) = extract_serde_types(temp.path(), &base());
        assert!(types.contains_key("rust:example-app::config::Private"));
        assert!(edges.iter().any(|edge| edge.p == "serde_field"));
    }

    #[test]
    fn binding_parser_accepts_literal_strings_and_inline_comments() {
        let temp = tempfile::tempdir().expect("tempdir");
        write(
            temp.path(),
            ".phronesis/graph.toml",
            "[[generated_artifacts]]\nproducer = 'cue:module::#Value' # exact id\nartifact = \"config/value.json\" # generated\n",
        );
        let bindings = load_bindings(temp.path());
        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings[0].producer, "cue:module::#Value");
        assert_eq!(bindings[0].artifact, "config/value.json");
    }

    // Pins the section-awareness fix. The scanner used to absorb every later
    // `key = value` line into the last `[[generated_artifacts]]` block it saw,
    // so adding any new table to a graph.toml that already had bindings
    // silently rewrote the binding's fields — with no diagnostic, because a
    // rewritten-but-well-formed binding looks exactly like an authored one.
    #[test]
    fn an_unrelated_section_terminates_the_binding_instead_of_absorbing_its_keys() {
        let temp = tempfile::tempdir().expect("tempdir");
        write(
            temp.path(),
            ".phronesis/graph.toml",
            r#"[[generated_artifacts]]
producer = "cue:example.game::cue::export::export::#Manifest"
artifact = "config/manifest.yaml"

[ownership.rust]
enabled = true
provider = "ast"
include = ["src/**/*.rs"]
artifact = "not/a/binding.yaml"
consumer = "rust:not::a::consumer"
"#,
        );
        let bindings = load_bindings(temp.path());
        assert_eq!(bindings.len(), 1, "the ownership table is not a binding");
        assert_eq!(
            bindings[0].artifact, "config/manifest.yaml",
            "a key in a later section must not overwrite the binding"
        );
        assert_eq!(
            bindings[0].consumer, None,
            "a key in a later section must not add a consumer"
        );
        let ownership = super::super::ownership::config::load(temp.path())
            .expect("the ownership section parses alongside bindings");
        assert!(ownership.enabled, "the ownership section still parses");
        assert_eq!(
            ownership.include,
            vec!["src/**/*.rs".to_string()],
            "the ownership include list still parses"
        );
        assert!(
            declares_artifact(temp.path(), "config/manifest.yaml"),
            "the declared artifact is still recognised"
        );
        assert!(
            !declares_artifact(temp.path(), "not/a/binding.yaml"),
            "a path in another section is not a declared artifact"
        );
    }

    #[test]
    fn bounded_literal_producer_and_consumer_are_inferred() {
        let temp = tempfile::tempdir().expect("tempdir");
        write(
            temp.path(),
            "cue/export/manifest.cue",
            "package export\nvalue: 1\n",
        );
        write(temp.path(), "config/manifest.yaml", "game_name: demo\n");
        write(
            temp.path(),
            "src/config.rs",
            r#"#[derive(Deserialize)]
pub struct GameManifest { pub game_name: String }

fn build() {
    let output = Command::new("cue")
        .args(["export", "cue/export/manifest.cue", "--out", "yaml"])
        .output()?;
    std::fs::write("config/manifest.yaml", output.stdout)?;
}

fn load() {
    let text = std::fs::read_to_string("config/manifest.yaml")?;
    let manifest: GameManifest = serde_yaml::from_str(&text)?;
}
"#,
        );
        let mut edges = base();
        edges[0] = Edge::base(
            "declares_module",
            &[
                "cue/export/manifest.cue",
                "cue:example.game::cue::export::export",
            ],
            "cue/export/manifest.cue",
        );
        augment(temp.path(), &mut edges);
        assert!(edges.iter().any(|edge| edge.p == "generates"));
        assert!(edges.iter().any(|edge| edge.p == "deserializes"));
        assert!(edges.iter().any(|edge| edge.p == "maps_data_key"));
    }

    #[test]
    fn generic_serde_value_deserialization_uses_the_unique_enclosing_function_as_consumer() {
        let d = tempfile::tempdir().expect("tempdir");
        write(
            d.path(),
            "cue/export/parser.cue",
            "package export\n#Parser: {}\n",
        );
        write(d.path(), "config/parser_tables.yaml", "intents: []\n");
        write(
            d.path(),
            "build.rs",
            r#"
fn generate_intent_tables() {
    let source = std::fs::read_to_string("config/parser_tables.yaml")?;
    let yaml: serde_yaml::Value = serde_yaml::from_str(&source)?;
}
fn export_parser_tables() {
    Command::new("cue").arg("export").arg("cue/export/parser.cue").arg("--out=yaml");
    std::fs::write("config/parser_tables.yaml", output.stdout)?;
}
"#,
        );
        let mut edges = vec![
            Edge::base(
                "declares_module",
                &["cue/export/parser.cue", "cue:game::cue::export::export"],
                "cue/export/parser.cue",
            ),
            Edge::base(
                "graph_module",
                &["cue:game::cue::export::export"],
                "cue/export/parser.cue",
            ),
            Edge::base(
                "defines_fn",
                &["build.rs", "rust:game#build::generate_intent_tables"],
                "build.rs",
            ),
            Edge::base(
                "declares_module",
                &[
                    "config/parser_tables.yaml",
                    "yaml:project::config::parser_tables::doc:0",
                ],
                "config/parser_tables.yaml",
            ),
        ];
        augment(d.path(), &mut edges);
        assert!(edges.iter().any(|edge| {
            edge.p == "consumes_data"
                && edge.a
                    == [
                        "rust:game#build::generate_intent_tables",
                        "yaml:project::config::parser_tables::doc:0",
                    ]
        }));
        assert!(!edges.iter().any(|edge| edge.p == "deserializes"));
    }

    #[test]
    fn concat_literal_paths_are_inferred_as_one_static_path() {
        let temp = tempfile::tempdir().expect("tempdir");
        write(
            temp.path(),
            "cue/export/manifest.cue",
            "package export\nvalue: 1\n",
        );
        write(temp.path(), "config/manifest.yaml", "game_name: demo\n");
        write(
            temp.path(),
            "src/config.rs",
            r#"#[derive(Deserialize)]
pub struct GameManifest { pub game_name: String }

fn build() {
    let output = Command::new("cue")
        .args(["export", concat!("cue/export/", "manifest.cue"), "--out", "yaml"])
        .output()?;
    std::fs::write(concat!("config/", "manifest.yaml"), output.stdout)?;
}

fn load() {
    let text = std::fs::read_to_string(concat!("config/", "manifest.yaml"))?;
    let manifest: GameManifest = serde_yaml::from_str(&text)?;
}
"#,
        );
        let mut edges = base();
        edges[0] = Edge::base(
            "declares_module",
            &[
                "cue/export/manifest.cue",
                "cue:example.game::cue::export::export",
            ],
            "cue/export/manifest.cue",
        );
        augment(temp.path(), &mut edges);
        assert!(edges.iter().any(|edge| edge.p == "generates"));
        assert!(edges.iter().any(|edge| edge.p == "deserializes"));
    }

    #[test]
    fn log_and_panic_messages_ending_in_artifact_extensions_are_not_paths() {
        let paths = literal_paths(
            r#"
            std::fs::write("config/manifest.yaml", output.stdout)
                .expect("Failed to write manifest.yaml");
            println!("Exported base manifest.yaml");
            "#,
            &[".yaml", ".yml", ".json"],
        );
        assert_eq!(paths, BTreeSet::from(["config/manifest.yaml".to_string()]));
    }

    #[test]
    fn cargo_directives_and_format_templates_are_not_literal_paths() {
        let paths = literal_paths(
            r#"
            println!("cargo:rerun-if-changed=config/parser_tables.yaml");
            let manifest = format!("{}/manifest.yaml", root);
            let module = format!("{}/modules/{}.yaml", root, name);
            let actual = "config/manifest.yaml";
            "#,
            &[".yaml", ".yml", ".json"],
        );
        assert_eq!(paths, BTreeSet::from(["config/manifest.yaml".to_string()]));
    }

    #[test]
    fn tracked_literal_paths_are_normalized_to_repository_form() {
        assert_eq!(
            literal_paths(r#".arg("./cue/export/manifest.cue")"#, &[".cue"]),
            BTreeSet::from(["cue/export/manifest.cue".to_string()])
        );
    }

    #[test]
    fn dynamic_or_multiple_literal_paths_are_not_guessed() {
        let temp = tempfile::tempdir().expect("tempdir");
        write(temp.path(), "a.cue", "package a\n");
        write(temp.path(), "b.cue", "package b\n");
        write(temp.path(), "config/a.yaml", "a: 1\n");
        write(temp.path(), "config/b.yaml", "b: 1\n");
        write(
            temp.path(),
            "src/config.rs",
            r#"fn build() {
    let output = Command::new("cue").args(["export", "a.cue", "b.cue", "--out", "yaml"]).output()?;
    let path = if flag { "config/a.yaml" } else { "config/b.yaml" };
    std::fs::write(path, output.stdout)?;
}"#,
        );
        let mut edges = base();
        augment(temp.path(), &mut edges);
        assert!(!edges.iter().any(|edge| edge.p == "generates"));
    }

    #[test]
    fn explicit_bindings_accept_all_executable_language_consumers() {
        for consumer in [
            "rust:app::config::load_manifest",
            "python:app::config::load_manifest",
            "typescript:app::src::config::loadManifest",
            "lua:app::config::load_manifest",
            "cue:app::config::config",
            "json:app::config::consumer",
            "yaml:app::config::consumer::doc:0",
            "helm3:app::templates::consumer",
        ] {
            let temp = tempfile::tempdir().expect("tempdir");
            write(temp.path(), "config/manifest.json", "{\"name\":\"demo\"}");
            write(
                temp.path(),
                ".phronesis/graph.toml",
                &format!(
                    "[[generated_artifacts]]\nproducer = \"cue:example.game::cue::export::export::#Manifest\"\nartifact = \"config/manifest.json\"\nconsumer = \"{consumer}\"\n"
                ),
            );
            let mut edges = base();
            edges.retain(|edge| {
                edge.a
                    .first()
                    .is_none_or(|value| value != "config/manifest.yaml")
            });
            edges.push(Edge::base(
                "declares_module",
                &["config/manifest.json", "json:project::config::manifest"],
                "config/manifest.json",
            ));
            edges.push(Edge::base(
                "graph_function",
                &[consumer],
                "src/config.fixture",
            ));
            augment(temp.path(), &mut edges);
            assert!(edges.iter().any(|edge| {
                edge.p == "consumes_data" && edge.a == [consumer, "json:project::config::manifest"]
            }));
            assert!(!edges.iter().any(|edge| edge.p == "deserializes"));
            assert!(!edges.iter().any(|edge| edge.p == "maps_data_key"));
            assert!(!edges.iter().any(|edge| edge.p == "unconsumed_data_key"));
        }
    }

    #[test]
    fn code_as_configuration_links_rust_through_lua_without_data_keys() {
        let temp = tempfile::tempdir().expect("tempdir");
        write(
            temp.path(),
            "config/rules.lua",
            "return { enabled = true }\n",
        );
        write(
            temp.path(),
            ".phronesis/graph.toml",
            "[[generated_artifacts]]\nproducer = \"rust:app::build::emit_rules\"\nartifact = \"config/rules.lua\"\nconsumer = \"rust:app::runtime::load_rules\"\n",
        );
        let mut edges = base();
        edges.extend([
            Edge::base(
                "graph_function",
                &["rust:app::build::emit_rules"],
                "build.rs",
            ),
            Edge::base(
                "declares_module",
                &["config/rules.lua", "lua:app::config::rules"],
                "config/rules.lua",
            ),
            Edge::base(
                "graph_function",
                &["rust:app::runtime::load_rules"],
                "src/runtime.rs",
            ),
        ]);
        augment(temp.path(), &mut edges);
        assert!(edges.iter().any(|edge| {
            edge.p == "generates"
                && edge.a == ["rust:app::build::emit_rules", "lua:app::config::rules"]
        }));
        assert!(edges.iter().any(|edge| {
            edge.p == "consumes_data"
                && edge.a == ["rust:app::runtime::load_rules", "lua:app::config::rules"]
        }));
        assert!(!edges.iter().any(|edge| edge.p == "data_key"));
    }
}
