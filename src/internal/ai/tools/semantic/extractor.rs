use std::{collections::HashMap, error::Error, fmt, ops::Range};

use tree_sitter::{Language, Node, Parser, Query, QueryCursor, StreamingIterator, Tree};
use tree_sitter_rust::LANGUAGE as RUST;

#[derive(Debug, Clone, PartialEq)]
pub struct SemanticDocument {
    pub symbols: Vec<SemanticSymbol>,
    pub diagnostics: Vec<SemanticDiagnostic>,
    pub used_fallback: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticDiagnostic {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SemanticSymbol {
    pub name: String,
    pub qualified_name: String,
    pub kind: SymbolKind,
    pub signature: String,
    pub range: SemanticRange,
    pub selection_range: SemanticRange,
    pub byte_range: Range<usize>,
    pub scope: SemanticScope,
    pub confidence: f32,
    pub approximate: bool,
    pub container: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolKind {
    Function,
    Method,
    Struct,
    Enum,
    Trait,
    Module,
    Const,
    Static,
    TypeAlias,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SemanticScope {
    File,
    Module,
    Crate,
    Workspace,
    External,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SemanticRange {
    pub start: SemanticPosition,
    pub end: SemanticPosition,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SemanticPosition {
    pub line: usize,
    pub column: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SymbolSource {
    pub symbol: SemanticSymbol,
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SemanticError {
    ParserLanguage(String),
    ParseReturnedNoTree,
    Query(String),
}

impl fmt::Display for SemanticError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ParserLanguage(message) => {
                write!(f, "failed to initialize Rust parser: {message}")
            }
            Self::ParseReturnedNoTree => write!(f, "Rust parser returned no syntax tree"),
            Self::Query(message) => write!(f, "failed to run Rust semantic query: {message}"),
        }
    }
}

impl Error for SemanticError {}

#[derive(Debug, Clone, PartialEq)]
pub enum SemanticReadError {
    Extract(SemanticError),
    NotFound { query: String },
    Ambiguous { candidates: Vec<SemanticSymbol> },
}

impl fmt::Display for SemanticReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Extract(error) => write!(f, "{error}"),
            Self::NotFound { query } => write!(f, "Rust symbol not found: {query}"),
            Self::Ambiguous { candidates } => {
                write!(
                    f,
                    "Rust symbol query is ambiguous ({} candidates)",
                    candidates.len()
                )
            }
        }
    }
}

impl Error for SemanticReadError {}

impl From<SemanticError> for SemanticReadError {
    fn from(value: SemanticError) -> Self {
        Self::Extract(value)
    }
}

pub fn extract_rust_symbols(source: &str) -> Result<SemanticDocument, SemanticError> {
    let language = rust_language();
    let tree = parse_rust(source, &language)?;
    if tree.root_node().has_error() {
        return Ok(fallback_document(
            source,
            SemanticDiagnostic {
                code: "parse_error".to_string(),
                message: "tree-sitter-rust reported syntax errors; used textual fallback"
                    .to_string(),
            },
        ));
    }

    let mut document = SemanticDocument {
        symbols: query_rust_symbols(source, &language, &tree)?,
        diagnostics: Vec::new(),
        used_fallback: false,
    };
    mark_ambiguous_unqualified_symbols(&mut document.symbols);
    Ok(document)
}

pub fn read_rust_symbol(source: &str, query: &str) -> Result<SymbolSource, SemanticReadError> {
    let document = extract_rust_symbols(source)?;
    let matches: Vec<SemanticSymbol> = document
        .symbols
        .into_iter()
        .filter(|symbol| symbol.qualified_name == query || symbol.name == query)
        .collect();

    match matches.as_slice() {
        [] => Err(SemanticReadError::NotFound {
            query: query.to_string(),
        }),
        [symbol] => Ok(SymbolSource {
            source: source
                .get(symbol.byte_range.clone())
                .unwrap_or_default()
                .to_string(),
            symbol: symbol.clone(),
        }),
        _ => Err(SemanticReadError::Ambiguous {
            candidates: matches,
        }),
    }
}

fn rust_language() -> Language {
    RUST.into()
}

fn parse_rust(source: &str, language: &Language) -> Result<Tree, SemanticError> {
    let mut parser = Parser::new();
    parser
        .set_language(language)
        .map_err(|error| SemanticError::ParserLanguage(error.to_string()))?;
    parser
        .parse(source, None)
        .ok_or(SemanticError::ParseReturnedNoTree)
}

fn query_rust_symbols(
    source: &str,
    language: &Language,
    tree: &Tree,
) -> Result<Vec<SemanticSymbol>, SemanticError> {
    let query = Query::new(language, include_str!("query/rust.scm"))
        .map_err(|error| SemanticError::Query(error.to_string()))?;
    let capture_names = query.capture_names();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&query, tree.root_node(), source.as_bytes());
    let mut symbols = Vec::new();

    while let Some(query_match) = matches.next() {
        let mut symbol_node = None;
        let mut name_node = None;
        let mut captured_kind = None;

        for capture in query_match.captures {
            let capture_name = capture_names
                .get(capture.index as usize)
                .copied()
                .unwrap_or_default();
            match capture_name {
                "name" => name_node = Some(capture.node),
                "symbol.function" => {
                    symbol_node = Some(capture.node);
                    captured_kind = Some(SymbolKind::Function);
                }
                "symbol.struct" => {
                    symbol_node = Some(capture.node);
                    captured_kind = Some(SymbolKind::Struct);
                }
                "symbol.enum" => {
                    symbol_node = Some(capture.node);
                    captured_kind = Some(SymbolKind::Enum);
                }
                "symbol.trait" => {
                    symbol_node = Some(capture.node);
                    captured_kind = Some(SymbolKind::Trait);
                }
                "symbol.module" => {
                    symbol_node = Some(capture.node);
                    captured_kind = Some(SymbolKind::Module);
                }
                "symbol.const" => {
                    symbol_node = Some(capture.node);
                    captured_kind = Some(SymbolKind::Const);
                }
                "symbol.static" => {
                    symbol_node = Some(capture.node);
                    captured_kind = Some(SymbolKind::Static);
                }
                "symbol.type" => {
                    symbol_node = Some(capture.node);
                    captured_kind = Some(SymbolKind::TypeAlias);
                }
                _ => {}
            }
        }

        let Some(symbol_node) = symbol_node else {
            continue;
        };
        let Some(name_node) = name_node else {
            continue;
        };
        let Some(kind) = captured_kind else {
            continue;
        };
        let Some(symbol) = build_symbol(source, symbol_node, name_node, kind) else {
            continue;
        };
        symbols.push(symbol);
    }

    symbols.sort_by_key(|symbol| symbol.byte_range.start);
    Ok(symbols)
}

fn build_symbol(
    source: &str,
    symbol_node: Node<'_>,
    name_node: Node<'_>,
    captured_kind: SymbolKind,
) -> Option<SemanticSymbol> {
    let name = name_node.utf8_text(source.as_bytes()).ok()?.to_string();
    let container = nearest_container(symbol_node, source);
    let kind = if captured_kind == SymbolKind::Function
        && matches!(
            container.as_ref().map(|item| item.container_kind),
            Some(ContainerKind::Impl | ContainerKind::Trait)
        ) {
        SymbolKind::Method
    } else {
        captured_kind
    };
    let container_name = container.as_ref().map(|item| item.name.clone());
    let qualified_name = container_name
        .as_ref()
        .map(|container| format!("{container}::{name}"))
        .unwrap_or_else(|| name.clone());
    let scope = container
        .as_ref()
        .map(|item| match item.container_kind {
            ContainerKind::Module => SemanticScope::Module,
            ContainerKind::Impl | ContainerKind::Trait => SemanticScope::File,
        })
        .unwrap_or(SemanticScope::File);

    Some(SemanticSymbol {
        name,
        qualified_name,
        kind,
        signature: signature_for_node(source, symbol_node),
        range: range_for_node(symbol_node),
        selection_range: range_for_node(name_node),
        byte_range: symbol_node.start_byte()..symbol_node.end_byte(),
        scope,
        confidence: 1.0,
        approximate: false,
        container: container_name,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContainerKind {
    Impl,
    Trait,
    Module,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Container {
    name: String,
    container_kind: ContainerKind,
}

fn nearest_container(mut node: Node<'_>, source: &str) -> Option<Container> {
    while let Some(parent) = node.parent() {
        match parent.kind() {
            "impl_item" => {
                let name = parent
                    .child_by_field_name("type")
                    .and_then(|item| item.utf8_text(source.as_bytes()).ok())
                    .map(str::trim)
                    .filter(|item| !item.is_empty())
                    .map(ToOwned::to_owned)?;
                return Some(Container {
                    name,
                    container_kind: ContainerKind::Impl,
                });
            }
            "trait_item" => {
                let name = parent
                    .child_by_field_name("name")
                    .and_then(|item| item.utf8_text(source.as_bytes()).ok())?
                    .to_string();
                return Some(Container {
                    name,
                    container_kind: ContainerKind::Trait,
                });
            }
            "mod_item" => {
                let name = parent
                    .child_by_field_name("name")
                    .and_then(|item| item.utf8_text(source.as_bytes()).ok())?
                    .to_string();
                return Some(Container {
                    name,
                    container_kind: ContainerKind::Module,
                });
            }
            _ => node = parent,
        }
    }
    None
}

fn signature_for_node(source: &str, node: Node<'_>) -> String {
    let start = node.start_byte();
    let end = node
        .child_by_field_name("body")
        .map(|body| body.start_byte())
        .or_else(|| {
            source
                .get(start..node.end_byte())
                .and_then(|text| text.find('{').or_else(|| text.find(';')))
                .map(|offset| start + offset)
        })
        .unwrap_or_else(|| node.end_byte());

    source
        .get(start..end)
        .unwrap_or_default()
        .trim()
        .trim_end_matches('{')
        .trim()
        .to_string()
}

fn range_for_node(node: Node<'_>) -> SemanticRange {
    SemanticRange {
        start: position_for_point(node.start_position()),
        end: position_for_point(node.end_position()),
    }
}

fn position_for_point(point: tree_sitter::Point) -> SemanticPosition {
    SemanticPosition {
        line: point.row + 1,
        column: point.column,
    }
}

fn mark_ambiguous_unqualified_symbols(symbols: &mut [SemanticSymbol]) {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for symbol in symbols.iter() {
        *counts.entry(symbol.name.clone()).or_default() += 1;
    }

    for symbol in symbols {
        if counts.get(&symbol.name).copied().unwrap_or_default() > 1 {
            symbol.approximate = true;
            symbol.confidence = 0.7;
        }
    }
}

fn fallback_document(source: &str, diagnostic: SemanticDiagnostic) -> SemanticDocument {
    SemanticDocument {
        symbols: fallback_textual_symbols(source),
        diagnostics: vec![diagnostic],
        used_fallback: true,
    }
}

fn fallback_textual_symbols(source: &str) -> Vec<SemanticSymbol> {
    let mut symbols = Vec::new();
    let mut byte_offset = 0;
    for (line_index, line) in source.lines().enumerate() {
        if let Some((name, column)) = fallback_function_name(line) {
            let name_end_column = column + name.len();
            let start = byte_offset;
            let end = start + line.len();
            symbols.push(SemanticSymbol {
                name: name.clone(),
                qualified_name: name,
                kind: SymbolKind::Function,
                signature: line.trim().to_string(),
                range: SemanticRange {
                    start: SemanticPosition {
                        line: line_index + 1,
                        column,
                    },
                    end: SemanticPosition {
                        line: line_index + 1,
                        column: line.len(),
                    },
                },
                selection_range: SemanticRange {
                    start: SemanticPosition {
                        line: line_index + 1,
                        column,
                    },
                    end: SemanticPosition {
                        line: line_index + 1,
                        column: name_end_column,
                    },
                },
                byte_range: start..end,
                scope: SemanticScope::File,
                confidence: 0.4,
                approximate: true,
                container: None,
            });
        }
        byte_offset += line.len() + 1;
    }
    symbols
}

fn fallback_function_name(line: &str) -> Option<(String, usize)> {
    const PREFIXES: &[&str] = &[
        "pub fn ",
        "pub(crate) fn ",
        "pub(super) fn ",
        "pub async fn ",
        "async fn ",
        "fn ",
    ];
    let leading = line.len() - line.trim_start().len();
    let trimmed = line.trim_start();
    for prefix in PREFIXES {
        if let Some(rest) = trimmed.strip_prefix(prefix) {
            let name: String = rest
                .chars()
                .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '_')
                .collect();
            if !name.is_empty() {
                return Some((name, leading + prefix.len()));
            }
        }
    }
    None
}
