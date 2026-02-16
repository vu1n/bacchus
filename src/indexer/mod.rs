//! AST Indexer module for Bacchus
//!
//! Uses tree-sitter for native AST parsing across multiple languages.

mod extractor;
mod parser;
mod types;

pub use extractor::extract_symbols;
pub use parser::Parser;
pub use types::{ExtractedSymbol, Language};
