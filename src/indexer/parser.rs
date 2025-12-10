//! Tree-sitter parser wrapper with multi-language support

use super::types::Language;
use thiserror::Error;
use tree_sitter::{Parser as TsParser, Tree};

#[derive(Error, Debug)]
pub enum ParserError {
    #[error("Failed to initialize parser: {0}")]
    InitError(String),
    #[error("Failed to parse source code")]
    ParseError,
    #[error("Unsupported language: {0}")]
    UnsupportedLanguage(String),
}

/// Multi-language tree-sitter parser
pub struct Parser {
    ts_parser: TsParser,
}

impl Parser {
    /// Create a new parser instance
    pub fn new() -> Result<Self, ParserError> {
        let ts_parser = TsParser::new();
        Ok(Parser { ts_parser })
    }

    /// Parse source code for a given language
    pub fn parse(&mut self, source: &str, language: Language) -> Result<Tree, ParserError> {
        let ts_language = match language {
            Language::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT,
            Language::JavaScript => tree_sitter_javascript::LANGUAGE,
            Language::Python => tree_sitter_python::LANGUAGE,
            Language::Go => tree_sitter_go::LANGUAGE,
            Language::Rust => tree_sitter_rust::LANGUAGE,
        };

        self.ts_parser
            .set_language(&ts_language.into())
            .map_err(|e| ParserError::InitError(e.to_string()))?;

        self.ts_parser
            .parse(source, None)
            .ok_or(ParserError::ParseError)
    }

    /// Parse a file, detecting language from extension
    pub fn parse_file(&mut self, source: &str, file_path: &str) -> Result<(Tree, Language), ParserError> {
        let ext = file_path
            .rsplit('.')
            .next()
            .unwrap_or("");

        let language = Language::from_extension(ext)
            .ok_or_else(|| ParserError::UnsupportedLanguage(ext.to_string()))?;

        let tree = self.parse(source, language)?;
        Ok((tree, language))
    }
}

impl Default for Parser {
    fn default() -> Self {
        Self::new().expect("Failed to create parser")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_typescript() {
        let mut parser = Parser::new().unwrap();
        let source = r#"
function hello(name: string): string {
    return `Hello, ${name}!`;
}
"#;
        let tree = parser.parse(source, Language::TypeScript).unwrap();
        assert!(!tree.root_node().has_error());
    }

    #[test]
    fn test_parse_python() {
        let mut parser = Parser::new().unwrap();
        let source = r#"
def hello(name: str) -> str:
    return f"Hello, {name}!"
"#;
        let tree = parser.parse(source, Language::Python).unwrap();
        assert!(!tree.root_node().has_error());
    }

    #[test]
    fn test_parse_rust() {
        let mut parser = Parser::new().unwrap();
        let source = r#"
fn hello(name: &str) -> String {
    format!("Hello, {}!", name)
}
"#;
        let tree = parser.parse(source, Language::Rust).unwrap();
        assert!(!tree.root_node().has_error());
    }

    #[test]
    fn test_parse_file_detection() {
        let mut parser = Parser::new().unwrap();
        let ts_source = "const x: number = 42;";

        let (tree, lang) = parser.parse_file(ts_source, "test.ts").unwrap();
        assert_eq!(lang, Language::TypeScript);
        assert!(!tree.root_node().has_error());
    }
}
