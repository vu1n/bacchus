//! Types for the indexer module

use serde::{Deserialize, Serialize};

/// Supported programming languages
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    TypeScript,
    JavaScript,
    Python,
    Go,
    Rust,
}

impl Language {
    /// Detect language from file extension
    pub fn from_extension(ext: &str) -> Option<Self> {
        match ext.to_lowercase().as_str() {
            "ts" | "tsx" => Some(Language::TypeScript),
            "js" | "jsx" | "mjs" | "cjs" => Some(Language::JavaScript),
            "py" => Some(Language::Python),
            "go" => Some(Language::Go),
            "rs" => Some(Language::Rust),
            _ => None,
        }
    }

    /// Get the language name as a string
    pub fn as_str(&self) -> &'static str {
        match self {
            Language::TypeScript => "typescript",
            Language::JavaScript => "javascript",
            Language::Python => "python",
            Language::Go => "go",
            Language::Rust => "rust",
        }
    }
}

impl std::fmt::Display for Language {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Symbol kinds that can be extracted from code
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SymbolKind {
    Function,
    Class,
    Method,
    Interface,
    Type,
    Variable,
    Struct,
    Enum,
    Trait,
    Impl,
}

impl SymbolKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            SymbolKind::Function => "function",
            SymbolKind::Class => "class",
            SymbolKind::Method => "method",
            SymbolKind::Interface => "interface",
            SymbolKind::Type => "type",
            SymbolKind::Variable => "variable",
            SymbolKind::Struct => "struct",
            SymbolKind::Enum => "enum",
            SymbolKind::Trait => "trait",
            SymbolKind::Impl => "impl",
        }
    }
}

impl std::fmt::Display for SymbolKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// An extracted symbol from source code
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractedSymbol {
    /// File path relative to workspace root
    pub file: String,
    /// Fully qualified name (file::module::Class::method)
    pub fq_name: String,
    /// Symbol kind
    pub kind: SymbolKind,
    /// Start line (1-indexed)
    pub span_start_line: u32,
    /// End line (1-indexed)
    pub span_end_line: u32,
    /// Number of lines
    pub line_count: u32,
    /// SHA256 hash of the symbol body
    pub hash: String,
    /// Documentation string (if present)
    pub docstring: Option<String>,
    /// Programming language
    pub language: Language,
}
