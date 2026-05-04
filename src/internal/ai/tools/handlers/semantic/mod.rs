//! Semantic code-understanding tool handlers.
//!
//! AI user story: give the agent AST-backed ways to inspect symbols and likely
//! relationships before falling back to raw text search. The current
//! implementation is Rust-only and file-scoped; approximate relationship tools
//! always report confidence and scope so callers do not mistake them for full
//! compiler analysis.

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::Arc,
};

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};

use super::parse_arguments;
use crate::internal::ai::tools::{
    ToolRegistryBuilder,
    context::{ToolInvocation, ToolKind, ToolOutput, ToolPayload},
    error::ToolError,
    registry::ToolHandler,
    semantic::{
        SemanticDocument, SemanticPosition, SemanticRange, SemanticReadError, SemanticScope,
        SemanticSymbol, SymbolKind, extract_rust_symbols, language_for_path, read_rust_symbol,
    },
    spec::{FunctionParameters, ToolSpec},
    utils::{
        generated_build_artifact_hidden_message, is_generated_build_artifact_path, resolve_path,
    },
};

const MAX_RELATION_LIMIT: usize = 200;
const DEFAULT_RELATION_LIMIT: usize = 100;
const MAX_TRACE_DEPTH: usize = 3;
const DEFAULT_TRACE_DEPTH: usize = 2;

/// Handler for listing Rust symbols in a file.
pub struct ListSymbolsHandler;

/// Handler for reading a single Rust symbol by name or qualified name.
pub struct ReadSymbolHandler;

/// Handler for approximate file-scoped reference search.
pub struct FindReferencesHandler;

/// Handler for approximate file-scoped caller tracing.
pub struct TraceCallersHandler;

#[derive(Debug, Deserialize)]
struct SemanticFileArgs {
    #[serde(alias = "path")]
    file_path: String,
}

#[derive(Debug, Deserialize)]
struct SymbolLookupArgs {
    #[serde(alias = "path")]
    file_path: String,
    #[serde(alias = "query", alias = "name")]
    symbol: String,
}

#[derive(Debug, Deserialize)]
struct SymbolRelationArgs {
    #[serde(alias = "path")]
    file_path: String,
    #[serde(alias = "query", alias = "name")]
    symbol: String,
    #[serde(default = "default_relation_limit")]
    limit: usize,
}

#[derive(Debug, Deserialize)]
struct TraceCallersArgs {
    #[serde(alias = "path")]
    file_path: String,
    #[serde(alias = "query", alias = "name")]
    symbol: String,
    #[serde(default = "default_trace_depth")]
    max_depth: usize,
    #[serde(default = "default_relation_limit")]
    limit: usize,
}

fn default_relation_limit() -> usize {
    DEFAULT_RELATION_LIMIT
}

fn default_trace_depth() -> usize {
    DEFAULT_TRACE_DEPTH
}

/// Register all semantic handlers on a tool registry builder.
pub fn register_semantic_handlers(builder: ToolRegistryBuilder) -> ToolRegistryBuilder {
    builder
        .register("list_symbols", Arc::new(ListSymbolsHandler))
        .register("read_symbol", Arc::new(ReadSymbolHandler))
        .register("find_references", Arc::new(FindReferencesHandler))
        .register("trace_callers", Arc::new(TraceCallersHandler))
}

#[async_trait]
impl ToolHandler for ListSymbolsHandler {
    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    async fn handle(&self, invocation: ToolInvocation) -> Result<ToolOutput, ToolError> {
        let (args, working_dir) = parse_function_args::<SemanticFileArgs>(invocation)?;
        let (path, source) = read_rust_source(&args.file_path, &working_dir).await?;
        let document = extract_rust_symbols(&source).map_err(semantic_extract_error)?;
        json_success(list_symbols_output(&path, &document))
    }

    fn schema(&self) -> ToolSpec {
        ToolSpec::new(
            "list_symbols",
            "List Rust symbols in one source file with ranges, scope, confidence, and approximate flags.",
        )
        .with_parameters(FunctionParameters::object(
            [(
                "file_path",
                "string",
                "Rust source file path, absolute or relative to the working directory",
            )],
            [("file_path", true)],
        ))
    }
}

#[async_trait]
impl ToolHandler for ReadSymbolHandler {
    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    async fn handle(&self, invocation: ToolInvocation) -> Result<ToolOutput, ToolError> {
        let (args, working_dir) = parse_function_args::<SymbolLookupArgs>(invocation)?;
        validate_symbol_query(&args.symbol)?;
        let (path, source) = read_rust_source(&args.file_path, &working_dir).await?;

        match read_rust_symbol(&source, args.symbol.trim()) {
            Ok(symbol_source) => json_success(json!({
                "status": "ok",
                "file_path": path.display().to_string(),
                "symbol": symbol_json(&symbol_source.symbol),
                "source": symbol_source.source,
            })),
            Err(SemanticReadError::Ambiguous { candidates }) => json_success(json!({
                "status": "ambiguous",
                "file_path": path.display().to_string(),
                "query": args.symbol.trim(),
                "candidates": candidates.iter().map(symbol_json).collect::<Vec<_>>(),
            })),
            Err(SemanticReadError::NotFound { query }) => json_success(json!({
                "status": "not_found",
                "file_path": path.display().to_string(),
                "query": query,
                "candidates": [],
            })),
            Err(SemanticReadError::Extract(error)) => Err(semantic_extract_error(error)),
        }
    }

    fn schema(&self) -> ToolSpec {
        ToolSpec::new(
            "read_symbol",
            "Read the source for one Rust symbol by name or qualified name. Ambiguous names return candidates instead of guessing.",
        )
        .with_parameters(FunctionParameters::object(
            [
                (
                    "file_path",
                    "string",
                    "Rust source file path, absolute or relative to the working directory",
                ),
                (
                    "symbol",
                    "string",
                    "Symbol name or qualified name, for example make_widget or Widget::label",
                ),
            ],
            [("file_path", true), ("symbol", true)],
        ))
    }
}

#[async_trait]
impl ToolHandler for FindReferencesHandler {
    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    async fn handle(&self, invocation: ToolInvocation) -> Result<ToolOutput, ToolError> {
        let (args, working_dir) = parse_function_args::<SymbolRelationArgs>(invocation)?;
        validate_symbol_query(&args.symbol)?;
        validate_limit(args.limit)?;
        let limit = args.limit.min(MAX_RELATION_LIMIT);
        let (path, source) = read_rust_source(&args.file_path, &working_dir).await?;
        let references = find_reference_candidates(&source, args.symbol.trim(), limit);

        json_success(json!({
            "file_path": path.display().to_string(),
            "symbol": args.symbol.trim(),
            "scope": scope_name(SemanticScope::File),
            "approximate": true,
            "confidence": relation_confidence(&references),
            "references": references,
            "truncated": references.len() >= limit,
        }))
    }

    fn schema(&self) -> ToolSpec {
        ToolSpec::new(
            "find_references",
            "Find likely references to a Rust symbol within one file. Results are approximate and include confidence metadata.",
        )
        .with_parameters(FunctionParameters::object(
            [
                (
                    "file_path",
                    "string",
                    "Rust source file path, absolute or relative to the working directory",
                ),
                (
                    "symbol",
                    "string",
                    "Symbol name or qualified name to search for",
                ),
                (
                    "limit",
                    "integer",
                    "Maximum reference candidates to return (default: 100, max: 200)",
                ),
            ],
            [("file_path", true), ("symbol", true)],
        ))
    }
}

#[async_trait]
impl ToolHandler for TraceCallersHandler {
    fn kind(&self) -> ToolKind {
        ToolKind::Function
    }

    async fn handle(&self, invocation: ToolInvocation) -> Result<ToolOutput, ToolError> {
        let (args, working_dir) = parse_function_args::<TraceCallersArgs>(invocation)?;
        validate_symbol_query(&args.symbol)?;
        validate_limit(args.limit)?;
        if args.max_depth == 0 {
            return Err(ToolError::InvalidArguments(
                "max_depth must be greater than zero".to_string(),
            ));
        }

        let max_depth = args.max_depth.min(MAX_TRACE_DEPTH);
        let limit = args.limit.min(MAX_RELATION_LIMIT);
        let (path, source) = read_rust_source(&args.file_path, &working_dir).await?;
        let document = extract_rust_symbols(&source).map_err(semantic_extract_error)?;
        let callers = trace_callers(&source, &document, args.symbol.trim(), max_depth, limit);

        json_success(json!({
            "file_path": path.display().to_string(),
            "symbol": args.symbol.trim(),
            "max_depth": max_depth,
            "scope": scope_name(SemanticScope::File),
            "approximate": true,
            "confidence": relation_confidence(&callers),
            "callers": callers,
            "truncated": callers.len() >= limit,
        }))
    }

    fn schema(&self) -> ToolSpec {
        ToolSpec::new(
            "trace_callers",
            "Trace likely Rust callers within one file. Depth is capped at 3 and results are approximate.",
        )
        .with_parameters(FunctionParameters::object(
            [
                (
                    "file_path",
                    "string",
                    "Rust source file path, absolute or relative to the working directory",
                ),
                (
                    "symbol",
                    "string",
                    "Target symbol name or qualified name to trace callers for",
                ),
                (
                    "max_depth",
                    "integer",
                    "Maximum caller depth to inspect (default: 2, max: 3)",
                ),
                (
                    "limit",
                    "integer",
                    "Maximum caller candidates to return (default: 100, max: 200)",
                ),
            ],
            [("file_path", true), ("symbol", true)],
        ))
    }
}

fn parse_function_args<T: serde::de::DeserializeOwned>(
    invocation: ToolInvocation,
) -> Result<(T, PathBuf), ToolError> {
    let ToolInvocation {
        payload,
        working_dir,
        tool_name,
        ..
    } = invocation;

    let ToolPayload::Function { arguments } = payload else {
        return Err(ToolError::IncompatiblePayload(format!(
            "{tool_name} handler only accepts Function payloads"
        )));
    };

    let args = parse_arguments(&arguments)?;
    Ok((args, working_dir))
}

async fn read_rust_source(
    file_path: &str,
    working_dir: &Path,
) -> Result<(PathBuf, String), ToolError> {
    let trimmed = file_path.trim();
    if trimmed.is_empty() {
        return Err(ToolError::InvalidArguments(
            "file_path must not be empty".to_string(),
        ));
    }

    let path = resolve_path(Path::new(trimmed), working_dir)?;
    if is_generated_build_artifact_path(&path, working_dir) {
        return Err(ToolError::InvalidArguments(
            generated_build_artifact_hidden_message(&path),
        ));
    }

    if language_for_path(&path).is_none() {
        return Err(ToolError::InvalidArguments(format!(
            "semantic tools currently support Rust .rs files only: {}",
            path.display()
        )));
    }

    let source = tokio::fs::read_to_string(&path).await.map_err(|error| {
        ToolError::ExecutionFailed(format!(
            "failed to read Rust source file '{}': {error}",
            path.display()
        ))
    })?;
    Ok((path, source))
}

fn validate_symbol_query(symbol: &str) -> Result<(), ToolError> {
    if symbol.trim().is_empty() {
        return Err(ToolError::InvalidArguments(
            "symbol must not be empty".to_string(),
        ));
    }
    Ok(())
}

fn validate_limit(limit: usize) -> Result<(), ToolError> {
    if limit == 0 {
        return Err(ToolError::InvalidArguments(
            "limit must be greater than zero".to_string(),
        ));
    }
    Ok(())
}

fn semantic_extract_error(error: impl std::fmt::Display) -> ToolError {
    ToolError::ExecutionFailed(format!("failed to extract Rust symbols: {error}"))
}

fn json_success(value: Value) -> Result<ToolOutput, ToolError> {
    let content = serde_json::to_string_pretty(&value).map_err(|error| {
        ToolError::ExecutionFailed(format!("failed to serialize semantic tool output: {error}"))
    })?;
    Ok(ToolOutput::success(content))
}

fn list_symbols_output(path: &Path, document: &SemanticDocument) -> Value {
    json!({
        "file_path": path.display().to_string(),
        "language": "rust",
        "scope": scope_name(SemanticScope::File),
        "used_fallback": document.used_fallback,
        "diagnostics": document.diagnostics.iter().map(|diagnostic| {
            json!({
                "code": diagnostic.code,
                "message": diagnostic.message,
            })
        }).collect::<Vec<_>>(),
        "symbols": document.symbols.iter().map(symbol_json).collect::<Vec<_>>(),
    })
}

fn symbol_json(symbol: &SemanticSymbol) -> Value {
    json!({
        "name": symbol.name,
        "qualified_name": symbol.qualified_name,
        "kind": symbol_kind_name(symbol.kind),
        "signature": symbol.signature,
        "range": range_json(symbol.range),
        "selection_range": range_json(symbol.selection_range),
        "byte_range": {
            "start": symbol.byte_range.start,
            "end": symbol.byte_range.end,
        },
        "scope": scope_name(symbol.scope),
        "confidence": symbol.confidence,
        "approximate": symbol.approximate,
        "container": symbol.container,
    })
}

fn range_json(range: SemanticRange) -> Value {
    json!({
        "start": position_json(range.start),
        "end": position_json(range.end),
    })
}

fn position_json(position: SemanticPosition) -> Value {
    json!({
        "line": position.line,
        "column": position.column,
    })
}

fn symbol_kind_name(kind: SymbolKind) -> &'static str {
    match kind {
        SymbolKind::Function => "function",
        SymbolKind::Method => "method",
        SymbolKind::Struct => "struct",
        SymbolKind::Enum => "enum",
        SymbolKind::Trait => "trait",
        SymbolKind::Module => "module",
        SymbolKind::Const => "const",
        SymbolKind::Static => "static",
        SymbolKind::TypeAlias => "type_alias",
    }
}

fn scope_name(scope: SemanticScope) -> &'static str {
    match scope {
        SemanticScope::File => "file",
        SemanticScope::Module => "module",
        SemanticScope::Crate => "crate",
        SemanticScope::Workspace => "workspace",
        SemanticScope::External => "external",
    }
}

fn find_reference_candidates(source: &str, symbol: &str, limit: usize) -> Vec<Value> {
    let needles = relation_needles(symbol);
    let mut references = Vec::new();
    let mut seen = HashSet::new();

    for (line_index, line) in source.lines().enumerate() {
        for needle in &needles {
            for column in find_needle_columns(line, needle) {
                if !seen.insert((line_index, column, needle.clone())) {
                    continue;
                }
                references.push(json!({
                    "line": line_index + 1,
                    "column": column,
                    "text": line.trim(),
                    "matched": needle,
                    "scope": scope_name(SemanticScope::File),
                    "confidence": reference_confidence(symbol, needle),
                    "approximate": true,
                }));
                if references.len() >= limit {
                    return references;
                }
            }
        }
    }

    references
}

fn trace_callers(
    source: &str,
    document: &SemanticDocument,
    symbol: &str,
    max_depth: usize,
    limit: usize,
) -> Vec<Value> {
    let mut callers = Vec::new();
    let mut seen_callers = HashSet::new();
    let mut targets = vec![symbol.to_string()];

    for depth in 1..=max_depth {
        let mut next_targets = Vec::new();
        for callable in callable_symbols(&document.symbols) {
            if seen_callers.contains(&callable.qualified_name) {
                continue;
            }
            if targets
                .iter()
                .any(|target| callable.qualified_name == *target || callable.name == *target)
            {
                continue;
            }

            let Some(callable_source) = source.get(callable.byte_range.clone()) else {
                continue;
            };
            let Some(matched) = first_relation_match(callable_source, &targets) else {
                continue;
            };

            seen_callers.insert(callable.qualified_name.clone());
            next_targets.push(callable.qualified_name.clone());
            callers.push(json!({
                "depth": depth,
                "symbol": symbol_json(callable),
                "matched": matched,
                "scope": scope_name(SemanticScope::File),
                "confidence": caller_confidence(&matched),
                "approximate": true,
            }));

            if callers.len() >= limit {
                return callers;
            }
        }

        if next_targets.is_empty() {
            break;
        }
        targets = next_targets;
    }

    callers
}

fn callable_symbols(symbols: &[SemanticSymbol]) -> impl Iterator<Item = &SemanticSymbol> {
    symbols
        .iter()
        .filter(|symbol| matches!(symbol.kind, SymbolKind::Function | SymbolKind::Method))
}

fn first_relation_match(source: &str, targets: &[String]) -> Option<String> {
    targets.iter().find_map(|target| {
        relation_needles(target)
            .into_iter()
            .find(|needle| source_contains_needle(source, needle))
    })
}

fn source_contains_needle(source: &str, needle: &str) -> bool {
    source
        .lines()
        .any(|line| !find_needle_columns(line, needle).is_empty())
}

fn relation_needles(symbol: &str) -> Vec<String> {
    let trimmed = symbol.trim();
    let mut needles = vec![trimmed.to_string()];
    if let Some(simple_name) = trimmed
        .rsplit("::")
        .next()
        .filter(|name| !name.is_empty() && *name != trimmed)
    {
        needles.push(simple_name.to_string());
    }
    needles
}

fn find_needle_columns(line: &str, needle: &str) -> Vec<usize> {
    if needle.is_empty() {
        return Vec::new();
    }

    let mut columns = Vec::new();
    let mut search_from = 0;
    while search_from <= line.len() {
        let Some(offset) = line[search_from..].find(needle) else {
            break;
        };
        let start = search_from + offset;
        let end = start + needle.len();
        if is_relation_boundary(line, start, end) {
            columns.push(start);
        }
        search_from = end;
    }
    columns
}

fn is_relation_boundary(line: &str, start: usize, end: usize) -> bool {
    let before = line[..start].chars().next_back();
    let after = line[end..].chars().next();
    !before.is_some_and(is_rust_identifier_char) && !after.is_some_and(is_rust_identifier_char)
}

fn is_rust_identifier_char(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphanumeric()
}

fn reference_confidence(symbol: &str, matched: &str) -> f64 {
    if matched == symbol.trim() { 0.75 } else { 0.55 }
}

fn caller_confidence(matched: &str) -> f64 {
    if matched.contains("::") { 0.7 } else { 0.55 }
}

fn relation_confidence(items: &[Value]) -> f64 {
    if items.is_empty() { 0.0 } else { 0.65 }
}
