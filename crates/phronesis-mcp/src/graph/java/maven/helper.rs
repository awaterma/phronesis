//! Build-helper configuration is merged before source roots are materialized.

use super::xml::{child, value};
use crate::graph::java::index::Context;
use roxmltree::Node;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Default)]
struct Configuration {
    name: String,
    text: String,
    attributes: BTreeMap<String, String>,
    children: Vec<Self>,
}

impl Configuration {
    fn parse(node: Node<'_, '_>) -> Self {
        Self {
            name: node.tag_name().name().into(),
            text: node.text().unwrap_or("").trim().into(),
            attributes: node
                .attributes()
                .map(|a| (a.name().into(), a.value().into()))
                .collect(),
            children: node
                .children()
                .filter(Node::is_element)
                .map(Self::parse)
                .collect(),
        }
    }

    /// Child configuration dominates; repeated children match in order.
    fn merge(&mut self, parent: &Self) {
        if self
            .attributes
            .get("combine.self")
            .is_some_and(|v| v == "override")
        {
            return;
        }
        for (key, value) in &parent.attributes {
            self.attributes
                .entry(key.clone())
                .or_insert_with(|| value.clone());
        }
        if self.text.is_empty() {
            self.text.clone_from(&parent.text);
        }
        if self
            .attributes
            .get("combine.children")
            .is_some_and(|v| v == "append")
        {
            let mut children = parent.children.clone();
            children.append(&mut self.children);
            self.children = children;
            return;
        }
        let original = self.children.len();
        let mut offsets = BTreeMap::<&str, usize>::new();
        for inherited in &parent.children {
            let matching = self.children[..original]
                .iter()
                .enumerate()
                .filter(|(_, node)| node.name == inherited.name)
                .map(|(index, _)| index)
                .collect::<Vec<_>>();
            let offset = offsets.entry(&inherited.name).or_default();
            if matching.is_empty() {
                self.children.push(inherited.clone());
            } else if let Some(index) = matching.get(*offset) {
                self.children[*index].merge(inherited);
            }
            *offset += 1;
        }
    }
}

fn merge_configuration(parent: &mut Option<Configuration>, child: &Option<Configuration>) {
    if let Some(mut dominant) = child.clone() {
        if let Some(inherited) = parent.as_ref() {
            dominant.merge(inherited);
        }
        *parent = Some(dominant);
    }
}

#[derive(Debug, Clone, Default)]
struct Execution {
    inherited: Option<bool>,
    phase: Option<String>,
    goals: BTreeSet<String>,
    configuration: Option<Configuration>,
}

#[derive(Debug, Clone, Default)]
pub(super) struct Helper {
    inherited: Option<bool>,
    configuration: Option<Configuration>,
    executions: BTreeMap<String, Execution>,
}

impl Helper {
    pub(super) fn parse(plugin: Node<'_, '_>) -> Self {
        let mut out = Self {
            inherited: value(plugin, "inherited").map(|v| v != "false"),
            configuration: child(plugin, "configuration").map(Configuration::parse),
            executions: BTreeMap::new(),
        };
        if let Some(executions) = child(plugin, "executions") {
            for execution in executions
                .children()
                .filter(|n| n.has_tag_name("execution"))
            {
                let goals = child(execution, "goals")
                    .map(|goals| {
                        goals
                            .children()
                            .filter(|n| n.has_tag_name("goal"))
                            .filter_map(|n| n.text().map(|s| s.trim().into()))
                            .collect()
                    })
                    .unwrap_or_default();
                out.executions.insert(
                    value(execution, "id").unwrap_or_else(|| "default".into()),
                    Execution {
                        inherited: value(execution, "inherited").map(|v| v != "false"),
                        phase: value(execution, "phase"),
                        goals,
                        configuration: child(execution, "configuration").map(Configuration::parse),
                    },
                );
            }
        }
        out
    }

    pub(super) fn inherit(&mut self) {
        self.executions
            .retain(|_, e| e.inherited.or(self.inherited) != Some(false));
        if self.inherited == Some(false) {
            self.configuration = None;
            self.inherited = None;
        }
    }

    pub(super) fn merge(&mut self, child: &Self) {
        self.inherited = child.inherited.or(self.inherited);
        merge_configuration(&mut self.configuration, &child.configuration);
        for (id, child) in &child.executions {
            let parent = self.executions.entry(id.clone()).or_default();
            parent.inherited = child.inherited.or(parent.inherited);
            if child.phase.is_some() {
                parent.phase.clone_from(&child.phase);
            }
            parent.goals.extend(child.goals.clone());
            merge_configuration(&mut parent.configuration, &child.configuration);
        }
    }

    pub(super) fn roots(&self) -> Vec<(Context, String)> {
        let mut roots = Vec::new();
        for execution in self
            .executions
            .values()
            .filter(|e| e.phase.as_deref() != Some("none"))
        {
            let mut configuration = self.configuration.clone();
            merge_configuration(&mut configuration, &execution.configuration);
            let Some(configuration) = configuration else {
                continue;
            };
            for goal in &execution.goals {
                let context = match goal.as_str() {
                    "add-source" => Context::Production,
                    "add-test-source" => Context::Test,
                    _ => continue,
                };
                if let Some(sources) = configuration.children.iter().find(|n| n.name == "sources") {
                    roots.extend(
                        sources
                            .children
                            .iter()
                            .filter(|n| n.name == "source" && !n.text.is_empty())
                            .map(|n| (context, n.text.clone())),
                    );
                }
            }
        }
        roots
    }
}
