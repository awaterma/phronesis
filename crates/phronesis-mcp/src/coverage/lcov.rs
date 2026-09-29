//! Import lcov line and function counters as per-test function-region evidence.
use crate::coverage::{
    collect::is_wanted_source,
    region_map::{FunctionSite, extract_function_sites_for},
    store::{COVERAGE_FORMAT, HitRecord},
};
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Component, Path, PathBuf},
};

#[derive(Debug, Default)]
pub struct LcovFile {
    pub test_name: Option<String>,
    pub files: Vec<LcovSource>,
}
#[derive(Debug, Default)]
pub struct LcovSource {
    pub path: String,
    pub function_hits: Vec<(String, u64)>,
    pub line_hits: Vec<(u64, u64)>,
}

pub fn parse_lcov(text: &str) -> Result<LcovFile> {
    let mut out = LcovFile::default();
    let mut current: Option<LcovSource> = None;
    for line in text.lines() {
        if let Some(t) = line.strip_prefix("TN:") {
            if !t.is_empty() {
                out.test_name = Some(t.to_string());
            }
        } else if let Some(sf) = line.strip_prefix("SF:") {
            if let Some(old) = current.take() {
                out.files.push(old);
            }
            current = Some(LcovSource {
                path: sf.into(),
                ..Default::default()
            });
        } else if let Some(v) = line.strip_prefix("FNDA:") {
            if let Some(src) = current.as_mut() {
                let (count, name) = v.split_once(',').context("invalid FNDA")?;
                src.function_hits.push((name.into(), count.parse()?));
            }
        } else if let Some(v) = line.strip_prefix("DA:") {
            if let Some(src) = current.as_mut() {
                let mut p = v.split(',');
                let line = p.next().context("invalid DA")?.parse()?;
                let count = p.next().context("invalid DA")?.parse()?;
                src.line_hits.push((line, count));
            }
        } else if line == "end_of_record"
            && let Some(old) = current.take()
        {
            out.files.push(old);
        }
    }
    if let Some(old) = current {
        out.files.push(old);
    }
    Ok(out)
}

#[derive(Debug, PartialEq, Eq)]
pub enum Relativized {
    Path(String),
    Missing,
    Ambiguous(Vec<String>),
}
fn clean(path: &Path) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => return None,
            Component::Normal(s) => out.push(s),
            Component::RootDir | Component::Prefix(_) => {}
        }
    }
    Some(out)
}
pub fn relativize(root: &Path, sf: &str) -> Relativized {
    let p = Path::new(sf);
    if !p.is_absolute() {
        return clean(p)
            .filter(|r| root.join(r).is_file())
            .map(|r| Relativized::Path(r.to_string_lossy().replace('\\', "/")))
            .unwrap_or(Relativized::Missing);
    }
    if let Ok(r) = p.strip_prefix(root) {
        return clean(r)
            .filter(|r| root.join(r).is_file())
            .map(|r| Relativized::Path(r.to_string_lossy().replace('\\', "/")))
            .unwrap_or(Relativized::Missing);
    }
    let parts: Vec<_> = p
        .components()
        .filter_map(|c| {
            if let Component::Normal(s) = c {
                Some(s)
            } else {
                None
            }
        })
        .collect();
    let mut candidates = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
                continue;
            }
            if !path.is_file() {
                continue;
            }
            let Ok(rel) = path.strip_prefix(root) else {
                continue;
            };
            let rel_parts: Vec<_> = rel
                .components()
                .filter_map(|c| {
                    if let Component::Normal(s) = c {
                        Some(s)
                    } else {
                        None
                    }
                })
                .collect();
            for n in 1..=rel_parts.len().min(parts.len()) {
                if rel_parts[rel_parts.len() - n..]
                    .iter()
                    .zip(&parts[parts.len() - n..])
                    .all(|(a, b)| a == b)
                {
                    candidates.push((n, rel.to_path_buf()));
                }
            }
        }
    }
    let Some(max) = candidates.iter().map(|x| x.0).max() else {
        return Relativized::Missing;
    };
    let best: Vec<_> = candidates
        .into_iter()
        .filter(|x| x.0 == max)
        .map(|x| x.1.to_string_lossy().replace('\\', "/"))
        .collect();
    if best.len() == 1 {
        Relativized::Path(best[0].clone())
    } else {
        Relativized::Ambiguous(best)
    }
}

#[derive(Debug, Deserialize)]
pub struct Manifest {
    pub revision: String,
    pub files: BTreeMap<String, String>,
}
impl Manifest {
    pub fn read(dir: &Path) -> Result<Self> {
        Ok(serde_json::from_slice(
            &std::fs::read(dir.join("manifest.json")).context("reading lcov manifest")?,
        )?)
    }
}

pub fn hit_sites<'a>(
    sites: &'a [FunctionSite],
    src: &LcovSource,
) -> (Vec<&'a FunctionSite>, Vec<&'a FunctionSite>) {
    let mut hit = Vec::new();
    let mut unattributable = Vec::new();
    for site in sites {
        if src.path.ends_with(".py") && site.body_start_line == site.start_line {
            match src.function_hits.iter().find(|(n, _)| n == site.name()) {
                Some((_, n)) if *n > 0 => hit.push(site),
                Some(_) => {}
                None => unattributable.push(site),
            }
        } else if src
            .line_hits
            .iter()
            .any(|(l, n)| *n > 0 && *l >= site.body_start_line && *l <= site.end_line)
        {
            hit.push(site);
        }
    }
    (hit, unattributable)
}

#[derive(Debug, Default)]
pub struct LcovDirSummary {
    pub files: usize,
    pub tests: usize,
    pub records: usize,
    pub unresolved_sf: Vec<String>,
    pub ambiguous_sf: Vec<String>,
    pub filtered_sf: Vec<String>,
    pub no_regions: Vec<String>,
    pub unattributable: Vec<String>,
}
pub fn records_from_lcov_dir(
    root: &Path,
    dir: &Path,
    tool: &str,
    revision: &str,
) -> Result<(Vec<HitRecord>, LcovDirSummary)> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            matches!(
                p.extension().and_then(|e| e.to_str()),
                Some("lcov" | "info")
            )
        })
        .collect();
    paths.sort();
    let mut summary = LcovDirSummary {
        files: paths.len(),
        ..Default::default()
    };
    let mut tests = BTreeSet::new();
    let mut cache: BTreeMap<String, Vec<FunctionSite>> = BTreeMap::new();
    let mut records = Vec::new();
    for path in paths {
        let l = parse_lcov(&std::fs::read_to_string(&path)?)
            .with_context(|| format!("parsing {}", path.display()))?;
        let test = l
            .test_name
            .with_context(|| format!("{}: missing TN", path.display()))?;
        tests.insert(test.clone());
        for src in l.files {
            let rel = match relativize(root, &src.path) {
                Relativized::Path(p) => p,
                Relativized::Missing => {
                    summary.unresolved_sf.push(src.path);
                    continue;
                }
                Relativized::Ambiguous(c) => {
                    summary
                        .ambiguous_sf
                        .push(format!("{} -> {}", src.path, c.join(" | ")));
                    continue;
                }
            };
            if !is_wanted_source(&rel) {
                summary.filtered_sf.push(rel);
                continue;
            }
            if !cache.contains_key(&rel) {
                let text = std::fs::read_to_string(root.join(&rel))?;
                cache.insert(rel.clone(), extract_function_sites_for(&rel, &text)?);
            }
            let sites = &cache[&rel];
            if sites.is_empty() {
                summary.no_regions.push(rel);
                continue;
            }
            let (hit, unattr) = hit_sites(sites, &src);
            summary
                .unattributable
                .extend(unattr.iter().map(|s| s.region_id(&rel)));
            for s in hit {
                records.push(HitRecord {
                    v: COVERAGE_FORMAT,
                    kind: "hit".into(),
                    test: test.clone(),
                    region: s.region_id(&rel),
                    file: rel.clone(),
                    start_line: s.start_line,
                    end_line: s.end_line,
                    hit_kind: "region".into(),
                    revision: revision.into(),
                    tool: tool.into(),
                });
            }
        }
    }
    summary.tests = tests.len();
    summary.records = records.len();
    for values in [
        &mut summary.unresolved_sf,
        &mut summary.ambiguous_sf,
        &mut summary.filtered_sf,
        &mut summary.no_regions,
        &mut summary.unattributable,
    ] {
        values.sort();
        values.dedup();
    }
    if records.is_empty() {
        bail!(
            "no coverage records: {} files, {} tests, {} unresolved, {} ambiguous, {} filtered, {} without regions",
            summary.files,
            summary.tests,
            summary.unresolved_sf.len(),
            summary.ambiguous_sf.len(),
            summary.filtered_sf.len(),
            summary.no_regions.len()
        );
    }
    Ok((records, summary))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parser_reads_test_source_function_and_line_data() {
        let f=parse_lcov("TN:python:pkg::tests::test_store::test_load\nSF:/w/pkg/store.py\nFNDA:1,load\nDA:1,1\nDA:2,0\nend_of_record\n").expect("parse");
        assert_eq!(
            f.test_name.as_deref(),
            Some("python:pkg::tests::test_store::test_load")
        );
        assert_eq!(f.files[0].function_hits, vec![("load".into(), 1)]);
        assert_eq!(f.files[0].line_hits, vec![(1, 1), (2, 0)]);
    }
    #[test]
    fn body_hits_ignore_imported_def_and_require_fnda_for_one_liners() {
        let sites = vec![
            FunctionSite {
                item_path: "load".into(),
                start_line: 1,
                body_start_line: 2,
                end_line: 2,
            },
            FunctionSite {
                item_path: "one".into(),
                start_line: 4,
                body_start_line: 4,
                end_line: 4,
            },
            FunctionSite {
                item_path: "two".into(),
                start_line: 5,
                body_start_line: 5,
                end_line: 5,
            },
        ];
        let src = LcovSource {
            path: "pkg/store.py".into(),
            function_hits: vec![("one".into(), 1)],
            line_hits: vec![(1, 1), (2, 1), (4, 1), (5, 1)],
        };
        let (hit, unattr) = hit_sites(&sites, &src);
        assert_eq!(
            hit.iter().map(|s| s.item_path.as_str()).collect::<Vec<_>>(),
            vec!["load", "one"]
        );
        assert_eq!(
            unattr
                .iter()
                .map(|s| s.item_path.as_str())
                .collect::<Vec<_>>(),
            vec!["two"]
        );
    }
    #[test]
    fn suffix_mapping_uses_longest_match_and_reports_collisions() {
        let d = tempfile::tempdir().expect("tempdir");
        let r = d.path();
        std::fs::create_dir_all(r.join("app/pkg")).expect("mkdir");
        std::fs::create_dir_all(r.join("pkg")).expect("mkdir");
        std::fs::write(r.join("app/pkg/x.py"), "").expect("file");
        std::fs::write(r.join("pkg/x.py"), "").expect("file");
        assert_eq!(
            relativize(r, "/work/app/pkg/x.py"),
            Relativized::Path("app/pkg/x.py".into())
        );
        assert!(matches!(
            relativize(r, "/else/foo/x.py"),
            Relativized::Ambiguous(_)
        ));
        assert_eq!(relativize(r, "../pkg/x.py"), Relativized::Missing);
    }
}
