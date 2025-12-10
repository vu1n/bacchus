//! Symbol extraction from AST nodes

use super::types::{ExtractedSymbol, Language, SymbolKind};
use sha2::{Digest, Sha256};
use tree_sitter::{Node, Tree};

/// Extract symbols from a parsed AST tree
pub fn extract_symbols(
    tree: &Tree,
    file_path: &str,
    source: &str,
    language: Language,
) -> Vec<ExtractedSymbol> {
    let mut symbols = Vec::new();
    let root = tree.root_node();

    extract_from_node(root, file_path, source, language, &[], &mut symbols);

    symbols
}

/// Recursively extract symbols from a node
fn extract_from_node(
    node: Node,
    file_path: &str,
    source: &str,
    language: Language,
    parent_names: &[String],
    symbols: &mut Vec<ExtractedSymbol>,
) {
    let (kind, name) = match language {
        Language::TypeScript | Language::JavaScript => extract_ts_symbol(&node, source),
        Language::Python => extract_python_symbol(&node, source),
        Language::Go => extract_go_symbol(&node, source),
        Language::Rust => extract_rust_symbol(&node, source),
    };

    let mut new_parent_names = parent_names.to_vec();

    if let (Some(kind), Some(name)) = (kind, name) {
        let start_line = node.start_position().row as u32 + 1;
        let end_line = node.end_position().row as u32 + 1;
        let line_count = end_line - start_line + 1;

        // Build fully qualified name
        let fq_name = if parent_names.is_empty() {
            format!("{}::{}", file_path, name)
        } else {
            format!("{}::{}::{}", file_path, parent_names.join("::"), name)
        };

        // Extract symbol body and compute hash
        let body = &source[node.start_byte()..node.end_byte()];
        let hash = compute_hash(body);

        // Extract docstring
        let docstring = extract_docstring(&node, source);

        symbols.push(ExtractedSymbol {
            file: file_path.to_string(),
            fq_name,
            kind,
            span_start_line: start_line,
            span_end_line: end_line,
            line_count,
            hash,
            docstring,
            language,
        });

        // Update parent names for nested symbols
        if matches!(kind, SymbolKind::Class | SymbolKind::Struct | SymbolKind::Trait | SymbolKind::Impl) {
            new_parent_names.push(name);
        }
    }

    // Recurse into children
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        extract_from_node(child, file_path, source, language, &new_parent_names, symbols);
    }
}

/// Extract TypeScript/JavaScript symbol info from a node
fn extract_ts_symbol(node: &Node, source: &str) -> (Option<SymbolKind>, Option<String>) {
    let kind = match node.kind() {
        "function_declaration" | "function_signature" => Some(SymbolKind::Function),
        "class_declaration" => Some(SymbolKind::Class),
        "method_definition" | "method_signature" => Some(SymbolKind::Method),
        "interface_declaration" => Some(SymbolKind::Interface),
        "type_alias_declaration" => Some(SymbolKind::Type),
        "lexical_declaration" => {
            // Check if it's an arrow function or const variable
            if let Some(declarator) = node.child_by_field_name("declarations") {
                if let Some(first_decl) = declarator.child(0) {
                    if let Some(value) = first_decl.child_by_field_name("value") {
                        if value.kind() == "arrow_function" || value.kind() == "function_expression" {
                            return (Some(SymbolKind::Function), get_node_name(&first_decl, source));
                        }
                    }
                    // Top-level variable
                    if node.parent().map(|p| p.kind()) == Some("program") {
                        return (Some(SymbolKind::Variable), get_node_name(&first_decl, source));
                    }
                }
            }
            None
        }
        _ => None,
    };

    let name = kind.and_then(|_| get_node_name(node, source));
    (kind, name)
}

/// Extract Python symbol info from a node
fn extract_python_symbol(node: &Node, source: &str) -> (Option<SymbolKind>, Option<String>) {
    let kind = match node.kind() {
        "function_definition" => Some(SymbolKind::Function),
        "class_definition" => Some(SymbolKind::Class),
        _ => None,
    };

    let name = kind.and_then(|_| get_node_name(node, source));

    // Check if function is a method (inside a class)
    if kind == Some(SymbolKind::Function) {
        if let Some(parent) = node.parent() {
            if parent.kind() == "block" {
                if let Some(grandparent) = parent.parent() {
                    if grandparent.kind() == "class_definition" {
                        return (Some(SymbolKind::Method), name);
                    }
                }
            }
        }
    }

    (kind, name)
}

/// Extract Go symbol info from a node
fn extract_go_symbol(node: &Node, source: &str) -> (Option<SymbolKind>, Option<String>) {
    let kind = match node.kind() {
        "function_declaration" => Some(SymbolKind::Function),
        "method_declaration" => Some(SymbolKind::Method),
        "type_declaration" => {
            // Check if it's a struct or interface
            if let Some(spec) = node.child_by_field_name("type_spec") {
                if let Some(type_node) = spec.child_by_field_name("type") {
                    match type_node.kind() {
                        "struct_type" => return (Some(SymbolKind::Struct), get_node_name(&spec, source)),
                        "interface_type" => return (Some(SymbolKind::Interface), get_node_name(&spec, source)),
                        _ => {}
                    }
                }
            }
            Some(SymbolKind::Type)
        }
        _ => None,
    };

    let name = kind.and_then(|_| get_node_name(node, source));
    (kind, name)
}

/// Extract Rust symbol info from a node
fn extract_rust_symbol(node: &Node, source: &str) -> (Option<SymbolKind>, Option<String>) {
    let kind = match node.kind() {
        "function_item" => Some(SymbolKind::Function),
        "struct_item" => Some(SymbolKind::Struct),
        "enum_item" => Some(SymbolKind::Enum),
        "trait_item" => Some(SymbolKind::Trait),
        "impl_item" => Some(SymbolKind::Impl),
        "type_item" => Some(SymbolKind::Type),
        _ => None,
    };

    let name = kind.and_then(|_| get_node_name(node, source));
    (kind, name)
}

/// Get the name identifier from a node
fn get_node_name(node: &Node, source: &str) -> Option<String> {
    // Try common field names
    for field in ["name", "identifier"] {
        if let Some(name_node) = node.child_by_field_name(field) {
            return Some(source[name_node.start_byte()..name_node.end_byte()].to_string());
        }
    }

    // Look for identifier child
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "identifier" || child.kind() == "type_identifier" {
            return Some(source[child.start_byte()..child.end_byte()].to_string());
        }
    }

    None
}

/// Extract docstring from preceding comments
fn extract_docstring(node: &Node, source: &str) -> Option<String> {
    let mut target = *node;

    // If inside export_statement, look before the export
    if let Some(parent) = node.parent() {
        if parent.kind() == "export_statement" {
            target = parent;
        }
    }

    let mut comments = Vec::new();
    let mut current = target.prev_sibling();

    // Collect consecutive comments
    while let Some(sibling) = current {
        if sibling.kind() == "comment" {
            let comment_text = source[sibling.start_byte()..sibling.end_byte()].trim().to_string();
            comments.insert(0, comment_text);

            // Check for more comments
            if let Some(prev) = sibling.prev_sibling() {
                if prev.kind() == "comment" {
                    let gap = sibling.start_position().row.saturating_sub(prev.end_position().row);
                    if gap <= 1 {
                        current = Some(prev);
                        continue;
                    }
                }
            }
        }
        break;
    }

    if comments.is_empty() {
        None
    } else {
        Some(comments.join("\n"))
    }
}

/// Compute SHA256 hash of text
fn compute_hash(text: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::indexer::Parser;

    #[test]
    fn test_extract_typescript_symbols() {
        let mut parser = Parser::new().unwrap();
        let source = r#"
/**
 * A greeting function
 */
export function hello(name: string): string {
    return `Hello, ${name}!`;
}

class Greeter {
    private name: string;

    greet(): void {
        console.log(`Hi, ${this.name}`);
    }
}
"#;
        let tree = parser.parse(source, Language::TypeScript).unwrap();
        let symbols = extract_symbols(&tree, "test.ts", source, Language::TypeScript);

        assert!(symbols.iter().any(|s| s.kind == SymbolKind::Function && s.fq_name.contains("hello")));
        assert!(symbols.iter().any(|s| s.kind == SymbolKind::Class && s.fq_name.contains("Greeter")));
        assert!(symbols.iter().any(|s| s.kind == SymbolKind::Method && s.fq_name.contains("greet")));
    }

    #[test]
    fn test_extract_rust_symbols() {
        let mut parser = Parser::new().unwrap();
        let source = r#"
/// A point in 2D space
struct Point {
    x: f64,
    y: f64,
}

impl Point {
    fn new(x: f64, y: f64) -> Self {
        Point { x, y }
    }
}

fn main() {
    let p = Point::new(1.0, 2.0);
}
"#;
        let tree = parser.parse(source, Language::Rust).unwrap();
        let symbols = extract_symbols(&tree, "test.rs", source, Language::Rust);

        assert!(symbols.iter().any(|s| s.kind == SymbolKind::Struct && s.fq_name.contains("Point")));
        assert!(symbols.iter().any(|s| s.kind == SymbolKind::Impl));
        assert!(symbols.iter().any(|s| s.kind == SymbolKind::Function && s.fq_name.contains("main")));
    }
}
