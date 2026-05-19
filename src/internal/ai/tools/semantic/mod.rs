//! Syntax-level semantic helpers for AI code-understanding tools.
//!
//! Boundary: this module is an AST-backed approximation layer. It exposes stable
//! ranges, confidence, and fallback metadata, but it is not a replacement for
//! rust-analyzer or a full crate resolver.

use std::path::Path;

pub mod extractor;

pub use extractor::{
    SemanticDiagnostic, SemanticDocument, SemanticPosition, SemanticRange, SemanticReadError,
    SemanticScope, SemanticSymbol, SymbolKind, SymbolSource, extract_rust_symbols,
    read_rust_symbol,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SemanticLanguage {
    Rust,
}

pub fn language_for_path(path: impl AsRef<Path>) -> Option<SemanticLanguage> {
    match path.as_ref().extension().and_then(|ext| ext.to_str()) {
        Some("rs") => Some(SemanticLanguage::Rust),
        _ => None,
    }
}
