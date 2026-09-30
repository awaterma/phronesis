use crate::coverage::lcov::{LcovSource, Relativized, relativize};
use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};

#[derive(Debug, Default)]
pub struct JacocoSummary {
    pub modules_without_source_root: Vec<String>,
    pub ambiguous: Vec<String>,
    pub kotlin_skipped: Vec<String>,
}
pub type TestSources = Vec<(String, Vec<LcovSource>)>;

pub fn jacoco_to_sources(
    xml: &str,
    map_path: &dyn Fn(&str, &str) -> Relativized,
) -> Result<(Vec<LcovSource>, Vec<String>)> {
    let doc = roxmltree::Document::parse(xml).context("parsing JaCoCo XML")?;
    let mut files = Vec::new();
    let mut skipped = Vec::new();
    for package in doc.descendants().filter(|n| n.has_tag_name("package")) {
        let pkg = package.attribute("name").unwrap_or("");
        for source in package.children().filter(|n| n.has_tag_name("sourcefile")) {
            let name = source
                .attribute("name")
                .context("JaCoCo sourcefile missing name")?;
            let path = match map_path(pkg, name) {
                Relativized::Path(path) => path,
                Relativized::Missing => {
                    skipped.push(format!("{pkg}/{name}: missing"));
                    continue;
                }
                Relativized::Ambiguous(paths) => {
                    skipped.push(format!("{pkg}/{name}: ambiguous ({})", paths.join(", ")));
                    continue;
                }
            };
            let mut item = LcovSource {
                path: path.clone(),
                ..Default::default()
            };
            for line in source.children().filter(|n| n.has_tag_name("line")) {
                let nr = line
                    .attribute("nr")
                    .context("JaCoCo line missing nr")?
                    .parse()?;
                let ci = line
                    .attribute("ci")
                    .context("JaCoCo line missing ci")?
                    .parse()?;
                item.line_hits.push((nr, ci));
            }
            for class in package.children().filter(|n| n.has_tag_name("class")) {
                let source_name = class.attribute("sourcefilename").unwrap_or("");
                if source_name != name {
                    continue;
                }
                for method in class.children().filter(|n| n.has_tag_name("method")) {
                    let method_name = method
                        .attribute("name")
                        .context("JaCoCo method missing name")?;
                    let method_name = method_name.replace("&lt;init&gt;", "<init>");
                    let covered = method
                        .children()
                        .find(|n| {
                            n.has_tag_name("counter") && n.attribute("type") == Some("METHOD")
                        })
                        .and_then(|n| n.attribute("covered"))
                        .context("JaCoCo method missing METHOD counter")?
                        .parse()?;
                    item.function_hits.push((method_name, covered));
                }
            }
            files.push(item);
        }
    }
    Ok((files, skipped))
}

pub fn read_jacoco_dir(root: &Path, dir: &Path) -> Result<(TestSources, JacocoSummary)> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.is_dir()
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.parse::<u32>().is_ok())
        })
        .collect();
    entries.sort();
    let mut out = Vec::new();
    let mut summary = JacocoSummary::default();
    for entry in entries {
        let xml_path = entry.join("jacoco.xml");
        let xml = std::fs::read_to_string(&xml_path)
            .with_context(|| format!("reading {}", xml_path.display()))?;
        let module_path = entry.join("module.txt");
        let module = std::fs::read_to_string(&module_path)
            .with_context(|| format!("reading {}", module_path.display()))?;
        let module = module.trim();
        let test = std::fs::read_to_string(entry.join("TN"))
            .context("reading JaCoCo TN")?
            .trim()
            .to_owned();
        if test.is_empty() {
            bail!("{}: empty TN", entry.display());
        }
        let module_missing = std::cell::Cell::new(false);
        let kotlin = std::cell::RefCell::new(Vec::new());
        let sources = jacoco_to_sources(&xml, &|pkg, file| {
            let suffix = format!("{}/{}", pkg, file);
            let java = Path::new(module).join("src/main/java").join(&suffix);
            let kt = Path::new(module).join("src/main/kotlin").join(&suffix);
            let java_exists = root.join(&java).is_file();
            let kt_exists = root.join(&kt).is_file();
            match (java_exists, kt_exists) {
                (true, true) => Relativized::Ambiguous(vec![
                    java.to_string_lossy().into(),
                    kt.to_string_lossy().into(),
                ]),
                (true, false) => Relativized::Path(java.to_string_lossy().replace('\\', "/")),
                (false, true) => {
                    kotlin.borrow_mut().push(kt.to_string_lossy().into_owned());
                    Relativized::Missing
                }
                (false, false) => {
                    module_missing.set(true);
                    relativize(root, &format!("/{suffix}"))
                }
            }
        })?;
        if module_missing.get() {
            summary.modules_without_source_root.push(module.to_owned());
        }
        summary.kotlin_skipped.extend(kotlin.into_inner());
        summary.ambiguous.extend(
            sources
                .1
                .iter()
                .filter(|s| s.contains("ambiguous"))
                .cloned(),
        );
        out.push((test, sources.0));
    }
    Ok((out, summary))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jacoco_lines_and_methods_become_lcov_source() {
        let xml = r#"<report><package name="com/x"><class name="com/x/Store" sourcefilename="Store.java"><method name="&lt;init&gt;"><counter type="METHOD" missed="0" covered="1"/></method><method name="load"><counter type="METHOD" missed="1" covered="0"/></method></class><sourcefile name="Store.java"><line nr="3" mi="0" ci="3"/></sourcefile></package></report>"#;
        let (sources, skipped) = jacoco_to_sources(xml, &|pkg, file| {
            Relativized::Path(format!("core/src/main/java/{pkg}/{file}"))
        })
        .expect("xml");
        assert!(skipped.is_empty());
        assert_eq!(sources[0].path, "core/src/main/java/com/x/Store.java");
        assert_eq!(sources[0].line_hits, vec![(3, 3)]);
        assert_eq!(
            sources[0].function_hits,
            vec![("<init>".into(), 1), ("load".into(), 0)]
        );
    }

    #[test]
    fn ambiguous_module_source_mapping_is_reported_and_not_first_wins() {
        let xml =
            r#"<report><package name="com/x"><sourcefile name="Store.java"/></package></report>"#;
        let (sources, skipped) = jacoco_to_sources(xml, &|_, _| {
            Relativized::Ambiguous(vec![
                "core/src/main/java/com/x/Store.java".into(),
                "api/src/main/java/com/x/Store.java".into(),
            ])
        })
        .expect("xml");
        assert!(sources.is_empty());
        assert_eq!(
            skipped,
            vec![
                "com/x/Store.java: ambiguous (core/src/main/java/com/x/Store.java, api/src/main/java/com/x/Store.java)"
            ]
        );
    }

    #[test]
    fn body_line_hits_match_loaded_method_but_not_unrun_overload_or_one_liner() {
        let xml = r#"<report><package name="com/x"><class name="com/x/Store" sourcefilename="Store.java"><method name="load"><counter type="METHOD" missed="0" covered="1"/></method><method name="load"><counter type="METHOD" missed="1" covered="0"/></method><method name="oneLiner"><counter type="METHOD" missed="1" covered="0"/></method></class><sourcefile name="Store.java"><line nr="6" mi="0" ci="2"/><line nr="9" mi="2" ci="0"/><line nr="11" mi="2" ci="0"/></sourcefile></package></report>"#;
        let sites = crate::coverage::language::java::java_function_sites("package com.x;\npublic class Store {\n public Store() {\n }\n public int load() {\n return 1;\n }\n public int load(int n) {\n return n;\n }\n public int oneLiner() { return 2; }\n}\n").expect("Java sites");
        let (sources, _) = jacoco_to_sources(xml, &|p, f| {
            Relativized::Path(format!("core/src/main/java/{p}/{f}"))
        })
        .expect("JaCoCo");
        let lang =
            crate::coverage::language::language_for_path("core/src/main/java/com/x/Store.java")
                .expect("Java row");
        let (hit, unattr) = crate::coverage::lcov::hit_sites(&sites, &sources[0], lang);
        assert_eq!(
            hit.iter().map(|s| s.item_path.as_str()).collect::<Vec<_>>(),
            vec!["Store::load"]
        );
        assert!(unattr.is_empty());
    }

    #[test]
    fn committed_java_store_fixture_maps_and_attributes_its_body_hits() {
        const XML: &str = include_str!("../../tests/fixtures/jacoco/java-store/cov/1/jacoco.xml");
        const SOURCE: &str = include_str!(
            "../../tests/fixtures/jacoco/java-store/core/src/main/java/com/x/Store.java"
        );
        let sites =
            crate::coverage::language::java::java_function_sites(SOURCE).expect("Java sites");
        let (sources, skipped) = jacoco_to_sources(XML, &|pkg, file| {
            Relativized::Path(format!("core/src/main/java/{pkg}/{file}"))
        })
        .expect("JaCoCo XML");
        assert!(skipped.is_empty());
        let language =
            crate::coverage::language::language_for_path(&sources[0].path).expect("Java row");
        let (hit, unattributable) = crate::coverage::lcov::hit_sites(&sites, &sources[0], language);
        assert_eq!(
            hit.iter()
                .map(|site| site.item_path.as_str())
                .collect::<Vec<_>>(),
            vec!["Store::<init>", "Store::load"]
        );
        assert!(unattributable.is_empty());
    }
}
