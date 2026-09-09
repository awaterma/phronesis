//! Content-validated parser reuse, bounded to eight repository roots.

use super::*;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

mod disk;

type Files = BTreeMap<String, (u64, Arc<parse::Source>)>;
static CACHE: OnceLock<Mutex<BTreeMap<PathBuf, Files>>> = OnceLock::new();

pub(super) fn parse_sources(
    root: &Path,
    inputs: &BTreeMap<String, String>,
) -> BTreeMap<String, Arc<parse::Source>> {
    let key = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let mut cache = CACHE.get_or_init(Default::default).lock().ok();
    if let Some(cache) = cache.as_mut()
        && !cache.contains_key(&key)
        && cache.len() >= 8
        && let Some(oldest) = cache.keys().next().cloned()
    {
        cache.remove(&oldest);
    }
    let previous = cache.as_ref().and_then(|cache| cache.get(&key));
    let persisted = previous.is_none().then(|| disk::load(&key)).flatten();
    let previous = previous.or(persisted.as_ref());
    let (next, out) = parse_inputs(inputs, previous);
    let changed = previous.is_none_or(|old| {
        old.len() != next.len()
            || next
                .iter()
                .any(|(file, (hash, _))| old.get(file).is_none_or(|(old_hash, _)| old_hash != hash))
    });
    if changed && (!next.is_empty() || previous.is_some_and(|old| !old.is_empty())) {
        disk::save(&key, &next);
    }
    if let Some(cache) = cache.as_mut() {
        cache.insert(key, next);
    }
    out
}

fn parse_inputs(
    inputs: &BTreeMap<String, String>,
    previous: Option<&Files>,
) -> (Files, BTreeMap<String, Arc<parse::Source>>) {
    let mut next = Files::new();
    let mut out = BTreeMap::new();
    for (file, body) in inputs.iter().filter(|(file, _)| file.ends_with(".java")) {
        let hash = crate::graph::sync::hash_content(body);
        let parsed = previous
            .and_then(|previous| previous.get(file))
            .filter(|(old, _)| *old == hash)
            .map(|(_, parsed)| parsed.clone())
            .unwrap_or_else(|| Arc::new(parse::parse(file, body)));
        next.insert(file.clone(), (hash, parsed.clone()));
        out.insert(file.clone(), parsed);
    }
    (next, out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retained_entries_reuse_only_identical_content() {
        let mut inputs = BTreeMap::from([("A.java".into(), "class A {}".into())]);
        let (cached, first) = parse_inputs(&inputs, None);
        let (_, warm) = parse_inputs(&inputs, Some(&cached));
        assert!(Arc::ptr_eq(&first["A.java"], &warm["A.java"]));
        inputs.insert("A.java".into(), "class B {}".into());
        let (_, changed) = parse_inputs(&inputs, Some(&cached));
        assert!(!Arc::ptr_eq(&first["A.java"], &changed["A.java"]));
        let (removed, sources) = parse_inputs(&BTreeMap::new(), Some(&cached));
        assert!(removed.is_empty());
        assert!(sources.is_empty());
    }

    #[test]
    fn repository_roots_do_not_share_same_named_source_state() {
        let first_root = tempfile::tempdir().expect("first repository");
        let second_root = tempfile::tempdir().expect("second repository");
        let first_inputs = BTreeMap::from([("A.java".into(), "package first; class A {}".into())]);
        let second_inputs =
            BTreeMap::from([("A.java".into(), "package second; class A {}".into())]);
        let first = parse_sources(first_root.path(), &first_inputs);
        let second = parse_sources(second_root.path(), &second_inputs);
        assert_eq!(first["A.java"].package, "first");
        assert_eq!(second["A.java"].package, "second");
        assert!(parse_sources(second_root.path(), &BTreeMap::new()).is_empty());
        let first_again = parse_sources(first_root.path(), &first_inputs);
        assert_eq!(first_again["A.java"].package, "first");
    }
}
