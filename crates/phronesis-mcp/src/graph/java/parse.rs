//! Tree-sitter extraction of Java declarations. Paths never supply names.

use super::index::Import;
use tree_sitter::{Node, Parser};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ImportDecl {
    Type(String),
    Wildcard(String),
    StaticMember(String),
    StaticWildcard(String),
    Module,
}

impl ImportDecl {
    pub fn as_import(&self) -> Import<'_> {
        match self {
            Self::Type(name) => Import::Type(name),
            Self::Wildcard(name) => Import::Wildcard(name),
            Self::StaticMember(name) => Import::StaticMember(name),
            Self::StaticWildcard(name) => Import::StaticWildcard(name),
            Self::Module => Import::Module,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Call {
    pub receiver: Option<String>,
    /// Exact type of a direct `new Type(...).method()` receiver, if known.
    pub constructed_type: Option<String>,
    pub name: String,
    pub arity: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Method {
    /// Member type path relative to the package, e.g. `Outer.Inner`.
    pub declaring_type: String,
    pub name: String,
    pub arity: usize,
    pub test_annotation: bool,
    pub is_static: bool,
    pub calls: Vec<Call>,
}

/// Cacheable declaration data, independent of build ownership and visibility.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Source {
    pub package: String,
    pub types: Vec<String>,
    pub imports: Vec<ImportDecl>,
    pub methods: Vec<Method>,
    pub skipped: usize,
    pub parse_failed: bool,
    pub module_info: bool,
    /// Conservatively exclude receiver spellings shadowed by value bindings.
    pub value_names: std::collections::BTreeSet<String>,
    /// Types with explicit ancestry whose inherited methods are not resolved.
    pub inherited_types: std::collections::BTreeSet<String>,
}

fn text<'a>(node: Node<'_>, source: &'a str) -> &'a str {
    node.utf8_text(source.as_bytes()).unwrap_or("")
}

fn is_comment(node: Node<'_>) -> bool {
    matches!(node.kind(), "line_comment" | "block_comment")
}

fn is_type(node: Node<'_>) -> bool {
    matches!(
        node.kind(),
        "class_declaration"
            | "interface_declaration"
            | "enum_declaration"
            | "record_declaration"
            | "annotation_type_declaration"
    )
}

/// Return identifier spelling without retaining comments around separators.
fn dotted_name(node: Node<'_>, source: &str) -> Option<String> {
    if node.has_error() || node.is_missing() {
        return None;
    }
    match node.kind() {
        "identifier" | "type_identifier" => Some(text(node, source).to_string()),
        "scoped_identifier" => {
            let scope = dotted_name(node.child_by_field_name("scope")?, source)?;
            let name = dotted_name(node.child_by_field_name("name")?, source)?;
            Some(format!("{scope}.{name}"))
        }
        "scoped_type_identifier" => {
            // Unlike scoped_identifier, the Java grammar does not name the
            // scope/name fields on a scoped type. Annotations can intervene.
            let scope = dotted_name(node.named_child(0)?, source)?;
            let name = dotted_name(
                node.named_child(node.named_child_count().checked_sub(1)?)?,
                source,
            )?;
            Some(format!("{scope}.{name}"))
        }
        "generic_type" => dotted_name(node.named_child(0)?, source),
        _ => None,
    }
}

fn leaves<'a>(node: Node<'_>, source: &'a str, out: &mut Vec<&'a str>) {
    if is_comment(node) {
        return;
    }
    if node.child_count() == 0 {
        out.push(text(node, source));
        return;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        leaves(child, source, out);
    }
}

fn module_identifier(token: &str) -> bool {
    // JLS 3.8/3.9: reserved words and literals cannot be identifiers.
    // Contextual words such as `module` and `record` remain valid here.
    // https://docs.oracle.com/javase/specs/jls/se25/html/jls-3.html#jls-3.9
    !matches!(
        token,
        "abstract"
            | "assert"
            | "boolean"
            | "break"
            | "byte"
            | "case"
            | "catch"
            | "char"
            | "class"
            | "const"
            | "continue"
            | "default"
            | "do"
            | "double"
            | "else"
            | "enum"
            | "extends"
            | "final"
            | "finally"
            | "float"
            | "for"
            | "goto"
            | "if"
            | "implements"
            | "import"
            | "instanceof"
            | "int"
            | "interface"
            | "long"
            | "native"
            | "new"
            | "package"
            | "private"
            | "protected"
            | "public"
            | "return"
            | "short"
            | "static"
            | "strictfp"
            | "super"
            | "switch"
            | "synchronized"
            | "this"
            | "throw"
            | "throws"
            | "transient"
            | "try"
            | "void"
            | "volatile"
            | "while"
            | "_"
            | "true"
            | "false"
            | "null"
    ) && token
        .chars()
        .next()
        .is_some_and(|c| c.is_alphabetic() || c == '_' || c == '$')
        && token
            .chars()
            .all(|c| c.is_alphanumeric() || c == '_' || c == '$')
}

fn import(node: Node<'_>, source: &str) -> Option<ImportDecl> {
    let mut tokens = Vec::new();
    leaves(node, source, &mut tokens);
    // Validate module names explicitly: grammar recovery can accept reserved
    // words here. Invalid module imports must not fall through as type imports.
    let mut cursor = node.walk();
    let module_import = node
        .children(&mut cursor)
        .any(|child| child.kind() == "module");
    if module_import {
        let ["import", "module", names @ .., ";"] = tokens.as_slice() else {
            return None;
        };
        return (!names.is_empty()
            && names.len() % 2 == 1
            && names.iter().enumerate().all(|(i, token)| {
                if i % 2 == 1 {
                    *token == "."
                } else {
                    module_identifier(token)
                }
            }))
        .then_some(ImportDecl::Module);
    }
    if node.has_error() {
        return None;
    }
    let mut cursor = node.walk();
    let name = node
        .named_children(&mut cursor)
        .find_map(|child| dotted_name(child, source))?;
    let static_import = tokens.contains(&"static");
    let wildcard = tokens.contains(&"*");
    Some(match (static_import, wildcard) {
        (false, false) => ImportDecl::Type(name),
        (false, true) => ImportDecl::Wildcard(name),
        (true, false) => ImportDecl::StaticMember(name),
        (true, true) => ImportDecl::StaticWildcard(name),
    })
}

fn arity(node: Option<Node<'_>>) -> usize {
    node.map_or(0, |node| {
        let mut cursor = node.walk();
        node.named_children(&mut cursor)
            .filter(|child| !is_comment(*child) && child.kind() != "receiver_parameter")
            .count()
    })
}

fn test_annotation(method: Node<'_>, source: &str) -> bool {
    let mut cursor = method.walk();
    let Some(modifiers) = method
        .named_children(&mut cursor)
        .find(|child| child.kind() == "modifiers")
    else {
        return false;
    };
    let mut cursor = modifiers.walk();
    modifiers.named_children(&mut cursor).any(|child| {
        matches!(child.kind(), "annotation" | "marker_annotation")
            && child
                .child_by_field_name("name")
                .and_then(|name| dotted_name(name, source))
                .is_some_and(|name| {
                    matches!(
                        name.rsplit('.').next(),
                        Some("Test" | "ParameterizedTest" | "RepeatedTest" | "TestFactory")
                    )
                })
    })
}

fn constructed_type(mut node: Node<'_>, source: &str) -> Option<String> {
    while node.kind() == "parenthesized_expression" {
        node = node.named_child(0)?;
    }
    if node.kind() != "object_creation_expression"
        || node.child(0)?.kind() != "new"
        || node
            .named_children(&mut node.walk())
            .any(|child| child.kind() == "class_body")
    {
        return None;
    }
    let mut ty = node.child_by_field_name("type")?;
    if ty.kind() == "generic_type" {
        ty = ty.named_child(0)?;
    }
    dotted_name(ty, source)
}

fn calls(node: Node<'_>, source: &str, out: &mut Vec<Call>) {
    // Calls in another type's methods do not belong to the enclosing method.
    if is_type(node) || matches!(node.kind(), "class_body" | "lambda_expression") {
        return;
    }
    if node.kind() == "method_invocation"
        && let Some(name) = node.child_by_field_name("name")
    {
        out.push(Call {
            receiver: node
                .child_by_field_name("object")
                .map(|object| text(object, source).to_string()),
            constructed_type: node
                .child_by_field_name("object")
                .and_then(|object| constructed_type(object, source)),
            name: text(name, source).to_string(),
            arity: arity(node.child_by_field_name("arguments")),
        });
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        calls(child, source, out);
    }
}

fn members(node: Node<'_>, enclosing: &str, source: &str, out: &mut Source) {
    if is_type(node) {
        let Some(name) = node
            .child_by_field_name("name")
            .and_then(|name| dotted_name(name, source))
        else {
            out.skipped += 1;
            return;
        };
        let qualified = if enclosing.is_empty() {
            name
        } else {
            format!("{enclosing}.{name}")
        };
        out.types.push(qualified.clone());
        if matches!(node.kind(), "enum_declaration" | "record_declaration")
            || node.named_children(&mut node.walk()).any(|child| {
                matches!(
                    child.kind(),
                    "superclass" | "super_interfaces" | "extends_interfaces"
                )
            })
        {
            out.inherited_types.insert(qualified.clone());
        }
        if let Some(body) = node.child_by_field_name("body") {
            members(body, &qualified, source, out);
        }
    } else if matches!(
        node.kind(),
        "class_body"
            | "interface_body"
            | "enum_body"
            | "enum_body_declarations"
            | "annotation_type_body"
    ) {
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            members(child, enclosing, source, out);
        }
    } else if node.kind() == "method_declaration" {
        let Some(name) = node.child_by_field_name("name") else {
            out.skipped += 1;
            return;
        };
        let mut found_calls = Vec::new();
        if let Some(body) = node.child_by_field_name("body") {
            calls(body, source, &mut found_calls);
        }
        out.methods.push(Method {
            declaring_type: enclosing.to_string(),
            name: text(name, source).to_string(),
            arity: arity(node.child_by_field_name("parameters")),
            test_annotation: test_annotation(node, source),
            is_static: {
                let mut cursor = node.walk();
                node.named_children(&mut cursor)
                    .filter(|n| n.kind() == "modifiers")
                    .any(|modifiers| {
                        let mut cursor = modifiers.walk();
                        modifiers
                            .children(&mut cursor)
                            .any(|n| n.kind() == "modifier" && text(n, source) == "static")
                    })
            },
            calls: found_calls,
        });
    }
}

/// Parse source declarations once for reuse by discovery and extraction.
pub fn parse(file: &str, source: &str) -> Source {
    if file.rsplit('/').next() == Some("module-info.java") {
        return Source {
            module_info: true,
            ..Source::default()
        };
    }
    let mut parser = Parser::new();
    if parser.set_language(&super::grammar::language()).is_err() {
        return failed();
    }
    let Some(tree) = parser.parse(source, None) else {
        return failed();
    };
    let mut out = Source::default();
    let root = tree.root_node();
    let mut pending = vec![root];
    while let Some(node) = pending.pop() {
        if matches!(
            node.kind(),
            "variable_declarator"
                | "formal_parameter"
                | "catch_formal_parameter"
                | "spread_parameter"
                | "instanceof_expression"
                | "enhanced_for_statement"
        ) && let Some(name) = node.child_by_field_name("name")
        {
            out.value_names.insert(text(name, source).to_string());
        }
        if node.kind() == "record_pattern_component"
            && let Some(name) = node.named_child(node.named_child_count().saturating_sub(1))
            && name.kind() == "identifier"
        {
            out.value_names.insert(text(name, source).to_string());
        }
        let mut cursor = node.walk();
        pending.extend(node.named_children(&mut cursor));
    }
    let mut cursor = root.walk();
    let mut has_package = false;
    for node in root.named_children(&mut cursor) {
        if node.kind() == "import_declaration" {
            if let Some(import) = import(node, source) {
                out.imports.push(import);
                continue;
            }
            return failed();
        }
        if node.has_error() || node.is_error() || node.is_missing() {
            return failed();
        }
        if node.kind() == "package_declaration" {
            if has_package {
                return failed();
            }
            has_package = true;
            let mut cursor = node.walk();
            let Some(package) = node
                .named_children(&mut cursor)
                .find_map(|child| dotted_name(child, source))
            else {
                return failed();
            };
            out.package = package;
        } else {
            members(node, "", source, &mut out);
        }
    }
    out.types.sort();
    out
}

fn failed() -> Source {
    Source {
        skipped: 1,
        parse_failed: true,
        ..Source::default()
    }
}

#[cfg(test)]
mod tests;
