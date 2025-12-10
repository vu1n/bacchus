//! AST Indexer module for Bacchus
//!
//! Uses tree-sitter for native AST parsing across multiple languages.

mod parser;
mod extractor;
mod types;

pub use parser::Parser;
pub use extractor::extract_symbols;
pub use types::{ExtractedSymbol, Language, SymbolKind};
