use libra::internal::ai::tools::semantic::{
    SemanticReadError, SemanticScope, SymbolKind, extract_rust_symbols, read_rust_symbol,
};

const SAMPLE: &str = include_str!("data/ai_semantic/rust/sample.rs");

#[test]
fn ai_semantic_rust_lists_symbols_with_exact_ranges() {
    let document = extract_rust_symbols(SAMPLE).expect("fixture should parse");

    assert!(!document.used_fallback);
    assert!(document.diagnostics.is_empty());

    let make_widget = document
        .symbols
        .iter()
        .find(|symbol| symbol.qualified_name == "make_widget")
        .expect("make_widget symbol should be listed");
    assert_eq!(make_widget.kind, SymbolKind::Function);
    assert_eq!(make_widget.scope, SemanticScope::File);
    assert!(!make_widget.approximate);
    assert_eq!(make_widget.confidence, 1.0);
    assert_eq!(make_widget.range.start.line, 15);
    assert!(
        make_widget
            .signature
            .contains("pub fn make_widget(name: &str) -> Widget")
    );

    let method = document
        .symbols
        .iter()
        .find(|symbol| symbol.qualified_name == "Widget::label")
        .expect("impl method should be qualified by container");
    assert_eq!(method.kind, SymbolKind::Method);
    assert_eq!(method.container.as_deref(), Some("Widget"));
    assert_eq!(method.range.start.line, 10);
}

#[test]
fn ai_semantic_rust_reads_symbol_source_by_qualified_name() {
    let symbol = read_rust_symbol(SAMPLE, "Widget::label").expect("method should be readable");

    assert_eq!(symbol.symbol.qualified_name, "Widget::label");
    assert_eq!(symbol.symbol.range.start.line, 10);
    assert!(symbol.source.contains("fn label(&self) -> &str"));
    assert!(symbol.source.contains("&self.name"));
}

#[test]
fn ai_semantic_rust_reports_ambiguous_unqualified_names_with_candidates() {
    let err = read_rust_symbol(SAMPLE, "handle").expect_err("unqualified handle is ambiguous");

    match err {
        SemanticReadError::Ambiguous { candidates } => {
            let names: Vec<_> = candidates
                .iter()
                .map(|symbol| symbol.qualified_name.as_str())
                .collect();
            assert_eq!(names, vec!["handle", "nested::handle"]);
            assert!(candidates.iter().all(|symbol| symbol.approximate));
            assert!(candidates.iter().all(|symbol| symbol.confidence < 0.8));
        }
        other => panic!("expected ambiguous error, got {other:?}"),
    }
}

#[test]
fn ai_semantic_rust_falls_back_without_panicking_on_parse_errors() {
    let document = extract_rust_symbols("pub fn broken(\n").expect("fallback should be successful");

    assert!(document.used_fallback);
    assert!(
        document
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "parse_error")
    );

    let broken = document
        .symbols
        .iter()
        .find(|symbol| symbol.name == "broken")
        .expect("fallback should still expose obvious function candidates");
    assert!(broken.approximate);
    assert!(broken.confidence < 0.5);
}
