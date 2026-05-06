use std::{fs, path::Path};

use libra::internal::ai::tools::{
    ToolRegistryBuilder,
    context::{ToolInvocation, ToolOutput, ToolPayload},
    handlers::register_semantic_handlers,
};
use serde_json::Value;
use tempfile::TempDir;

const SAMPLE: &str = include_str!("data/ai_semantic/rust/sample.rs");

fn semantic_registry(root: &Path) -> libra::internal::ai::tools::ToolRegistry {
    register_semantic_handlers(ToolRegistryBuilder::with_working_dir(root.to_path_buf())).build()
}

fn write_sample(root: &Path) -> String {
    let src_dir = root.join("src");
    fs::create_dir_all(&src_dir).expect("fixture directory should be writable");
    let file_path = src_dir.join("lib.rs");
    fs::write(&file_path, SAMPLE).expect("fixture should be writable");
    file_path.display().to_string()
}

async fn dispatch_json(
    registry: &libra::internal::ai::tools::ToolRegistry,
    tool_name: &str,
    args: Value,
) -> Value {
    let output = registry
        .dispatch(ToolInvocation::new(
            "call-1",
            tool_name,
            ToolPayload::Function {
                arguments: args.to_string(),
            },
            registry.working_dir().to_path_buf(),
        ))
        .await
        .expect("tool call should succeed");
    let ToolOutput::Function { content, .. } = output else {
        panic!("semantic tools should return function output");
    };
    serde_json::from_str(&content).expect("semantic tool output should be JSON")
}

#[test]
fn semantic_registry_installs_all_semantic_tools() {
    let temp_dir = TempDir::new().expect("tempdir should be available");

    let registry = semantic_registry(temp_dir.path());

    for tool_name in [
        "list_symbols",
        "read_symbol",
        "find_references",
        "trace_callers",
    ] {
        assert!(
            registry.contains_tool(tool_name),
            "{tool_name} should be registered"
        );
    }
}

#[tokio::test]
async fn list_symbols_returns_confidence_scope_and_approximation_metadata() {
    let temp_dir = TempDir::new().expect("tempdir should be available");
    let file_path = write_sample(temp_dir.path());
    let registry = semantic_registry(temp_dir.path());

    let output = dispatch_json(
        &registry,
        "list_symbols",
        serde_json::json!({ "file_path": file_path }),
    )
    .await;

    let symbols = output["symbols"]
        .as_array()
        .expect("symbols should be an array");
    let make_widget = symbols
        .iter()
        .find(|symbol| symbol["qualified_name"] == "make_widget")
        .expect("make_widget should be listed");
    assert_eq!(make_widget["kind"], "function");
    assert_eq!(make_widget["scope"], "file");
    assert_eq!(make_widget["confidence"], 1.0);
    assert_eq!(make_widget["approximate"], false);

    let handle = symbols
        .iter()
        .find(|symbol| symbol["qualified_name"] == "handle")
        .expect("ambiguous unqualified handle should be listed");
    assert_eq!(handle["approximate"], true);
    assert!(handle["confidence"].as_f64().unwrap_or_default() < 0.8);
}

#[tokio::test]
async fn read_symbol_returns_exact_source_for_qualified_symbol() {
    let temp_dir = TempDir::new().expect("tempdir should be available");
    let file_path = write_sample(temp_dir.path());
    let registry = semantic_registry(temp_dir.path());

    let output = dispatch_json(
        &registry,
        "read_symbol",
        serde_json::json!({
            "file_path": file_path,
            "symbol": "Widget::label"
        }),
    )
    .await;

    assert_eq!(output["status"], "ok");
    assert_eq!(output["symbol"]["qualified_name"], "Widget::label");
    assert_eq!(output["symbol"]["kind"], "method");
    assert!(
        output["source"]
            .as_str()
            .unwrap_or_default()
            .contains("&self.name")
    );
}

#[tokio::test]
async fn find_references_returns_bounded_approximate_candidates() {
    let temp_dir = TempDir::new().expect("tempdir should be available");
    let file_path = write_sample(temp_dir.path());
    let registry = semantic_registry(temp_dir.path());

    let output = dispatch_json(
        &registry,
        "find_references",
        serde_json::json!({
            "file_path": file_path,
            "symbol": "Widget::new"
        }),
    )
    .await;

    assert_eq!(output["symbol"], "Widget::new");
    assert_eq!(output["scope"], "file");
    assert_eq!(output["approximate"], true);
    assert!(output["confidence"].as_f64().unwrap_or_default() < 1.0);

    let references = output["references"]
        .as_array()
        .expect("references should be an array");
    assert!(references.iter().any(|reference| {
        reference["text"]
            .as_str()
            .is_some_and(|text| text.contains("Widget::new(name)"))
    }));
}

#[tokio::test]
async fn trace_callers_caps_depth_and_reports_approximate_callers() {
    let temp_dir = TempDir::new().expect("tempdir should be available");
    let file_path = write_sample(temp_dir.path());
    let registry = semantic_registry(temp_dir.path());

    let output = dispatch_json(
        &registry,
        "trace_callers",
        serde_json::json!({
            "file_path": file_path,
            "symbol": "Widget::new",
            "max_depth": 99
        }),
    )
    .await;

    assert_eq!(output["symbol"], "Widget::new");
    assert_eq!(output["max_depth"], 3);
    assert_eq!(output["scope"], "file");
    assert_eq!(output["approximate"], true);

    let callers = output["callers"]
        .as_array()
        .expect("callers should be an array");
    assert!(callers.iter().any(|caller| {
        caller["symbol"]["qualified_name"] == "make_widget"
            && caller["confidence"].as_f64().unwrap_or_default() < 1.0
    }));
}
