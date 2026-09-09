//! Restricted BUILD evaluation over a Python syntax tree; no code executes.

use crate::graph::java::maven::Diagnostics;
use globset::GlobBuilder;
use std::collections::BTreeMap;
use tree_sitter::{Node, Parser};

#[derive(Debug, Clone)]
enum Value {
    String(String),
    List(Vec<String>),
    StringChoices(Vec<String>),
}

impl Value {
    fn list(self) -> Result<Vec<String>, ()> {
        match self {
            Self::List(list) => Ok(list),
            _ => Err(()),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub(super) struct Call {
    pub kind: String,
    pub name: String,
    pub srcs: Vec<String>,
    pub deps: Vec<String>,
    pub exports: Vec<String>,
    pub test_class: Option<String>,
    pub actual: Vec<String>,
}

struct Evaluator<'a> {
    file: &'a str,
    body: &'a str,
    files: &'a [String],
    bindings: BTreeMap<String, Value>,
    diagnostics: &'a mut Diagnostics,
}

fn text<'a>(node: Node<'_>, body: &'a str) -> &'a str {
    node.utf8_text(body.as_bytes()).unwrap_or("")
}

fn string(node: Node<'_>, body: &str) -> Result<String, ()> {
    let raw = text(node, body);
    let quote = raw.chars().next().ok_or(())?;
    if !matches!(quote, '\'' | '"') || !raw.ends_with(quote) || raw.len() < 2 {
        return Err(());
    }
    let mut out = String::new();
    let mut chars = raw[1..raw.len() - 1].chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next().ok_or(())? {
                '\\' => out.push('\\'),
                '\'' => out.push('\''),
                '"' => out.push('"'),
                'n' => out.push('\n'),
                'r' => out.push('\r'),
                't' => out.push('\t'),
                '\n' => {}
                _ => return Err(()),
            }
        } else if c == quote || matches!(c, '\n' | '\r') {
            return Err(());
        } else {
            out.push(c);
        }
    }
    Ok(out)
}

impl Evaluator<'_> {
    fn count(&mut self, name: &str, node: Node<'_>) {
        self.diagnostics
            .record(name, format!("{}:{}", self.file, node.start_byte()));
    }

    fn value(&mut self, node: Node<'_>, depth: usize) -> Result<Value, ()> {
        if depth > 128 || node.has_error() {
            return Err(());
        }
        match node.kind() {
            "string" => string(node, self.body).map(Value::String),
            "identifier" => {
                let name = text(node, self.body);
                if let Some(value) = self.bindings.get(name) {
                    return Ok(value.clone());
                }
                self.count("unbound_identifier", node);
                Ok(Value::List(Vec::new()))
            }
            "list" => {
                let mut out = Vec::new();
                let mut cursor = node.walk();
                for child in node
                    .named_children(&mut cursor)
                    .filter(|n| n.kind() != "comment")
                {
                    match self.value(child, depth + 1)? {
                        Value::String(value) => out.push(value),
                        Value::List(_) | Value::StringChoices(_) => return Err(()),
                    }
                }
                Ok(Value::List(out))
            }
            "binary_operator" => {
                let operator = node.child_by_field_name("operator").ok_or(())?;
                if text(operator, self.body) != "+" {
                    return Err(());
                }
                let left = self.value(node.child_by_field_name("left").ok_or(())?, depth + 1)?;
                let right = self.value(node.child_by_field_name("right").ok_or(())?, depth + 1)?;
                match (left, right) {
                    (Value::List(mut left), Value::List(right)) => {
                        left.extend(right);
                        Ok(Value::List(left))
                    }
                    (Value::String(mut left), Value::String(right)) => {
                        left.push_str(&right);
                        Ok(Value::String(left))
                    }
                    _ => Err(()),
                }
            }
            "call" => self.expression_call(node, depth + 1),
            _ => Err(()),
        }
    }

    fn expression_call(&mut self, node: Node<'_>, depth: usize) -> Result<Value, ()> {
        let name = text(node.child_by_field_name("function").ok_or(())?, self.body);
        let args = node.child_by_field_name("arguments").ok_or(())?;
        let mut cursor = args.walk();
        let args = args
            .named_children(&mut cursor)
            .filter(|n| n.kind() != "comment")
            .collect::<Vec<_>>();
        match name {
            "select" => {
                let [dictionary] = args.as_slice() else {
                    return Err(());
                };
                if dictionary.kind() != "dictionary" {
                    return Err(());
                }
                let mut values = Vec::new();
                let mut scalar = None;
                let mut cursor = dictionary.walk();
                for pair in dictionary
                    .named_children(&mut cursor)
                    .filter(|n| n.kind() != "comment")
                {
                    let key = pair.child_by_field_name("key").ok_or(())?;
                    string(key, self.body)?;
                    let (is_scalar, branch) = match self
                        .value(pair.child_by_field_name("value").ok_or(())?, depth + 1)?
                    {
                        Value::String(value) => (true, vec![value]),
                        Value::StringChoices(values) => (true, values),
                        Value::List(values) => (false, values),
                    };
                    if scalar.is_some_and(|previous| previous != is_scalar) {
                        return Err(());
                    }
                    scalar = Some(is_scalar);
                    values.extend(branch);
                }
                self.count("select_branch_unioned", node);
                if scalar == Some(true) {
                    Ok(Value::StringChoices(values))
                } else {
                    Ok(Value::List(values))
                }
            }
            "glob" => {
                let mut include = None;
                let mut exclude = Vec::new();
                for (i, arg) in args.iter().enumerate() {
                    if arg.kind() == "keyword_argument" {
                        let key = text(arg.child_by_field_name("name").ok_or(())?, self.body);
                        let value = self
                            .value(arg.child_by_field_name("value").ok_or(())?, depth + 1)?
                            .list()?;
                        match key {
                            "include" if include.is_none() => include = Some(value),
                            "exclude" => exclude = value,
                            _ => return Err(()),
                        }
                    } else if i == 0 {
                        include = Some(self.value(*arg, depth + 1)?.list()?);
                    } else {
                        return Err(());
                    }
                }
                let compile = |patterns: Vec<String>| {
                    patterns
                        .into_iter()
                        .map(|pattern| {
                            GlobBuilder::new(&pattern)
                                .literal_separator(true)
                                .build()
                                .map(|g| g.compile_matcher())
                                .map_err(|_| ())
                        })
                        .collect::<Result<Vec<_>, _>>()
                };
                let include = compile(include.ok_or(())?)?;
                let exclude = compile(exclude)?;
                Ok(Value::List(
                    self.files
                        .iter()
                        .filter(|file| {
                            include.iter().any(|pattern| pattern.is_match(file))
                                && !exclude.iter().any(|pattern| pattern.is_match(file))
                        })
                        .cloned()
                        .collect(),
                ))
            }
            _ => Err(()),
        }
    }

    fn target(&mut self, node: Node<'_>) -> Result<Call, ()> {
        let function = node.child_by_field_name("function").ok_or(())?;
        if function.kind() != "identifier" {
            return Err(());
        }
        let mut out = Call {
            kind: text(function, self.body).into(),
            ..Call::default()
        };
        let args = node.child_by_field_name("arguments").ok_or(())?;
        let mut cursor = args.walk();
        for arg in args.named_children(&mut cursor) {
            if arg.kind() != "keyword_argument" {
                continue;
            }
            let key = text(arg.child_by_field_name("name").ok_or(())?, self.body);
            // Other attributes do not affect ownership. Do not discard valid
            // srcs because e.g. timeout=30 lies outside the expression subset.
            if !matches!(
                key,
                "name" | "srcs" | "deps" | "exports" | "test_class" | "actual"
            ) {
                if matches!(key, "visibility" | "strict_deps") {
                    self.count("build_visibility_approximated", arg);
                }
                continue;
            }
            let value = self.value(arg.child_by_field_name("value").ok_or(())?, 0)?;
            match (key, value) {
                ("name", Value::String(name)) => out.name = name,
                ("test_class", Value::String(name)) => out.test_class = Some(name),
                ("actual", Value::String(name)) => out.actual = vec![name],
                ("actual", Value::StringChoices(names)) => out.actual = names,
                ("srcs", Value::List(values)) => out.srcs = values,
                ("deps", Value::List(values)) => out.deps = values,
                ("exports", Value::List(values)) => out.exports = values,
                _ => return Err(()),
            }
        }
        Ok(out)
    }
}

pub(super) fn evaluate(
    file: &str,
    body: &str,
    files: &[String],
    diagnostics: &mut Diagnostics,
) -> Vec<Call> {
    let mut parser = Parser::new();
    if parser
        .set_language(&tree_sitter_python::LANGUAGE.into())
        .is_err()
    {
        return Vec::new();
    }
    let Some(tree) = parser.parse(body, None) else {
        return Vec::new();
    };
    let mut eval = Evaluator {
        file,
        body,
        files,
        bindings: BTreeMap::new(),
        diagnostics,
    };
    let mut calls = Vec::new();
    let mut cursor = tree.root_node().walk();
    for statement in tree.root_node().named_children(&mut cursor) {
        if statement.kind() == "comment" {
            continue;
        }
        let Some(node) = statement
            .named_child(0)
            .filter(|_| statement.kind() == "expression_statement")
        else {
            eval.count("unsupported_syntax_skipped", statement);
            continue;
        };
        let result = if statement.has_error() {
            Err(())
        } else if node.kind() == "assignment" {
            let left = node.child_by_field_name("left");
            let right = node.child_by_field_name("right");
            match (left, right) {
                (Some(left), Some(right)) if left.kind() == "identifier" => {
                    let value = eval.value(right, 0);
                    match value {
                        Ok(value) => {
                            eval.bindings.insert(text(left, body).into(), value);
                            Ok(())
                        }
                        Err(()) => {
                            eval.bindings.remove(text(left, body));
                            Err(())
                        }
                    }
                }
                _ => Err(()),
            }
        } else if node.kind() == "call" {
            eval.target(node).map(|call| calls.push(call))
        } else {
            Err(())
        };
        if result.is_err() {
            eval.count("unsupported_syntax_skipped", statement);
        }
    }
    calls
}
