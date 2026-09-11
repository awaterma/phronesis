//! Owned subset of a POM needed for source ownership and compile visibility.

use super::helper::Helper;
use super::xml::{child, value};
use super::{Diagnostics, Scope};
use roxmltree::{Document, Node};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default)]
pub(super) struct Build {
    pub production: Option<String>,
    pub test: Option<String>,
    pub helper: Helper,
}

impl Build {
    pub(super) fn merge(&mut self, child: &Self) {
        if child.production.is_some() {
            self.production.clone_from(&child.production);
        }
        if child.test.is_some() {
            self.test.clone_from(&child.test);
        }
        self.helper.merge(&child.helper);
    }
}

#[derive(Debug, Clone)]
pub(super) struct Parent {
    pub group: Option<String>,
    pub artifact: Option<String>,
    pub path: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub(super) struct Pom {
    pub group: Option<String>,
    pub artifact: Option<String>,
    pub packaging: Option<String>,
    pub parent: Option<Parent>,
    pub build: Build,
    pub dependencies: BTreeMap<(String, String), Scope>,
}

fn object(file: &str, node: Node<'_, '_>) -> String {
    format!("{file}:{}", node.range().start)
}

fn dependencies(
    node: Node<'_, '_>,
    file: &str,
    diagnostics: &mut Diagnostics,
) -> BTreeMap<(String, String), Scope> {
    let mut out = BTreeMap::new();
    if let Some(management) = child(node, "dependencyManagement") {
        diagnostics.record("dependency_modifier_ignored", object(file, management));
    }
    let Some(dependencies) = child(node, "dependencies") else {
        return out;
    };
    for dependency in dependencies
        .children()
        .filter(|n| n.has_tag_name("dependency"))
    {
        for modifier in dependency
            .children()
            .filter(|n| n.has_tag_name("optional") || n.has_tag_name("exclusions"))
        {
            diagnostics.record("dependency_modifier_ignored", object(file, modifier));
        }
        let (Some(group), Some(artifact)) = (
            value(dependency, "groupId"),
            value(dependency, "artifactId"),
        ) else {
            diagnostics.record("dependency_identity_unresolved", object(file, dependency));
            continue;
        };
        let scope = Scope::parse(value(dependency, "scope").as_deref().unwrap_or("compile"));
        if value(dependency, "type").is_some_and(|kind| kind != "jar")
            || value(dependency, "classifier").is_some_and(|classifier| !classifier.is_empty())
            || matches!(scope, Scope::System | Scope::Unsupported)
        {
            // A reactor's main sources cannot stand in for an arbitrary
            // classifier or systemPath artifact merely sharing coordinates.
            diagnostics.record("dependency_artifact_unsupported", object(file, dependency));
            continue;
        }
        out.insert((group, artifact), scope);
    }
    out
}

fn build(node: Node<'_, '_>, file: &str, diagnostics: &mut Diagnostics) -> Build {
    let Some(build) = child(node, "build") else {
        return Build::default();
    };
    let mut out = Build {
        production: value(build, "sourceDirectory"),
        test: value(build, "testSourceDirectory"),
        helper: Helper::default(),
    };
    let Some(plugins) = child(build, "plugins") else {
        return out;
    };
    for plugin in plugins.children().filter(|n| n.has_tag_name("plugin")) {
        let helper = value(plugin, "artifactId").as_deref() == Some("build-helper-maven-plugin");
        if !helper {
            if plugin.descendants().any(|node| {
                node.is_element()
                    && matches!(
                        node.tag_name().name(),
                        "sourceDirectory"
                            | "testSourceDirectory"
                            | "sources"
                            | "sourceRoot"
                            | "outputDirectory"
                    )
            }) {
                diagnostics.record("unsupported_plugin_source_modifier", object(file, plugin));
            }
            continue;
        }
        out.helper.merge(&Helper::parse(plugin));
    }
    out
}

pub(super) fn parse(file: &str, body: &str, diagnostics: &mut Diagnostics) -> Option<Pom> {
    // roxmltree defaults reject DTDs, so external entities are never fetched.
    let document = Document::parse(body).ok()?;
    let project = document.root_element();
    if !project.has_tag_name("project") {
        return None;
    }
    let mut out = Pom {
        group: value(project, "groupId").filter(|s| !s.is_empty()),
        artifact: value(project, "artifactId").filter(|s| !s.is_empty()),
        packaging: value(project, "packaging"),
        parent: child(project, "parent").map(|node| Parent {
            group: value(node, "groupId").filter(|s| !s.is_empty()),
            artifact: value(node, "artifactId").filter(|s| !s.is_empty()),
            path: value(node, "relativePath"),
        }),
        build: build(project, file, diagnostics),
        dependencies: dependencies(project, file, diagnostics),
    };
    if let Some(profiles) = child(project, "profiles") {
        for profile in profiles.children().filter(|n| n.has_tag_name("profile")) {
            let active = child(profile, "activation")
                .and_then(|node| value(node, "activeByDefault"))
                .is_some_and(|value| value == "true");
            if active {
                out.build.merge(&build(profile, file, diagnostics));
                out.dependencies
                    .extend(dependencies(profile, file, diagnostics));
            } else {
                diagnostics.record("inactive_profile_skipped", object(file, profile));
            }
        }
    }
    Some(out)
}
