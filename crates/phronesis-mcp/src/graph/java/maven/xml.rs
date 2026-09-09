//! Namespace-neutral accessors shared by POM and plugin parsing.

use roxmltree::Node;

pub(super) fn child<'a, 'input>(node: Node<'a, 'input>, name: &str) -> Option<Node<'a, 'input>> {
    node.children()
        .find(|child| child.is_element() && child.tag_name().name() == name)
}

pub(super) fn value(node: Node<'_, '_>, name: &str) -> Option<String> {
    child(node, name).map(|child| child.text().unwrap_or("").trim().to_string())
}
