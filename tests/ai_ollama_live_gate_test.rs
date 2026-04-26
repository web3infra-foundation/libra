//! Local-only Phase 0 live gate for Ollama.
//!
//! Run manually with:
//! `LIBRA_AI_LIVE_OLLAMA=1 OLLAMA_HOST=http://127.0.0.1:11434 cargo test --test ai_ollama_live_gate_test`

use std::{sync::Arc, time::Duration};

use libra::internal::ai::{
    mcp::server::LibraMcpServer,
    tools::{
        ToolRegistryBuilder,
        handlers::{
            ApplyPatchHandler, GrepFilesHandler, ListDirHandler, McpBridgeHandler, PlanHandler,
            ReadFileHandler, RequestUserInputHandler, SearchFilesHandler, ShellHandler,
            SubmitIntentDraftHandler, SubmitPlanDraftHandler,
        },
    },
};
use serde::Deserialize;
use serde_json::{Value, json};

const REQUIRED_MODEL: &str = "gemma4:31b";

fn live_ollama_enabled() -> bool {
    std::env::var("LIBRA_AI_LIVE_OLLAMA").is_ok_and(|value| value == "1")
}

fn ollama_host() -> String {
    std::env::var("OLLAMA_HOST").unwrap_or_else(|_| "http://127.0.0.1:11434".to_string())
}

#[derive(Debug, Deserialize)]
struct TagsResponse {
    models: Vec<OllamaModel>,
}

#[derive(Debug, Deserialize)]
struct OllamaModel {
    name: String,
}

#[tokio::test]
async fn local_ollama_has_required_model_and_generates_minimal_response() {
    if !live_ollama_enabled() {
        eprintln!("skipped (set LIBRA_AI_LIVE_OLLAMA=1 to run local Ollama gate)");
        return;
    }

    let host = ollama_host();
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()
        .expect("build reqwest client");

    let tags: TagsResponse = client
        .get(format!("{host}/api/tags"))
        .send()
        .await
        .expect("query Ollama tags")
        .error_for_status()
        .expect("Ollama tags status")
        .json()
        .await
        .expect("decode Ollama tags");

    assert!(
        tags.models.iter().any(|model| model.name == REQUIRED_MODEL),
        "Ollama model {REQUIRED_MODEL} is not installed"
    );

    let response: serde_json::Value = client
        .post(format!("{host}/api/chat"))
        .json(&json!({
            "model": REQUIRED_MODEL,
            "messages": [
                {
                    "role": "user",
                    "content": "Say OK."
                }
            ],
            "stream": false,
            "options": {
                "num_predict": 64,
                "temperature": 0
            }
        }))
        .send()
        .await
        .expect("send Ollama chat request")
        .error_for_status()
        .expect("Ollama chat status")
        .json()
        .await
        .expect("decode Ollama chat response");

    let text = response
        .pointer("/message/content")
        .and_then(|value| value.as_str())
        .unwrap_or_default()
        .trim();
    assert!(!text.is_empty(), "Ollama returned an empty response");
}

#[tokio::test]
async fn local_ollama_accepts_libra_tool_schemas() {
    if !live_ollama_enabled() {
        eprintln!("skipped (set LIBRA_AI_LIVE_OLLAMA=1 to run local Ollama gate)");
        return;
    }

    let host = ollama_host();
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()
        .expect("build reqwest client");
    let tools = libra_code_tool_schemas();

    let mut failures = Vec::new();
    for tool in &tools {
        let name = tool
            .pointer("/function/name")
            .and_then(Value::as_str)
            .unwrap_or("<unknown>");
        let response = client
            .post(format!("{host}/api/chat"))
            .json(&json!({
                "model": REQUIRED_MODEL,
                "messages": [
                    {
                        "role": "user",
                        "content": "Do not call tools. Say OK."
                    }
                ],
                "stream": false,
                "tools": [tool],
                "options": {
                    "num_predict": 8,
                    "temperature": 0
                }
            }))
            .send()
            .await
            .unwrap_or_else(|error| panic!("send Ollama chat request for tool {name}: {error}"));
        let status = response.status();
        let body = response
            .text()
            .await
            .unwrap_or_else(|error| format!("<failed to read body: {error}>"));
        if !status.is_success() {
            failures.push(format!("{name}: status {}: {body}", status.as_u16()));
        }
    }

    assert!(
        failures.is_empty(),
        "Ollama rejected Libra tool schemas:\n{}",
        failures.join("\n")
    );

    let default_tools = tools
        .iter()
        .filter(|tool| {
            tool.pointer("/function/name").and_then(Value::as_str) != Some("submit_intent_draft")
        })
        .cloned()
        .collect::<Vec<_>>();
    assert_ollama_accepts_tool_set(&client, &host, "default tool set", default_tools).await;
    let plan_tools = tools
        .iter()
        .filter(|tool| {
            matches!(
                tool.pointer("/function/name").and_then(Value::as_str),
                Some(
                    "read_file"
                        | "list_dir"
                        | "grep_files"
                        | "search_files"
                        | "request_user_input"
                        | "submit_intent_draft"
                        | "submit_plan_draft"
                )
            )
        })
        .cloned()
        .collect::<Vec<_>>();
    assert_ollama_accepts_tool_set(&client, &host, "plan tool set", plan_tools).await;
}

fn libra_code_tool_schemas() -> Vec<Value> {
    let (user_input_tx, _user_input_rx) = tokio::sync::mpsc::unbounded_channel::<
        libra::internal::ai::tools::context::UserInputRequest,
    >();
    let mut builder = ToolRegistryBuilder::with_working_dir(std::path::PathBuf::from("/tmp"))
        .register("read_file", Arc::new(ReadFileHandler))
        .register("list_dir", Arc::new(ListDirHandler))
        .register("grep_files", Arc::new(GrepFilesHandler))
        .register("search_files", Arc::new(SearchFilesHandler))
        .register("apply_patch", Arc::new(ApplyPatchHandler))
        .register("shell", Arc::new(ShellHandler))
        .register("update_plan", Arc::new(PlanHandler))
        .register("submit_intent_draft", Arc::new(SubmitIntentDraftHandler))
        .register("submit_plan_draft", Arc::new(SubmitPlanDraftHandler))
        .register(
            "request_user_input",
            Arc::new(RequestUserInputHandler::new(user_input_tx)),
        );
    let mcp_server = Arc::new(LibraMcpServer::new(None, None));
    for (name, handler) in McpBridgeHandler::all_handlers(mcp_server.clone()) {
        builder = builder.register(name, handler);
    }
    builder.build().tool_specs_json()
}

async fn assert_ollama_accepts_tool_set(
    client: &reqwest::Client,
    host: &str,
    label: &str,
    tools: Vec<Value>,
) {
    let response = client
        .post(format!("{host}/api/chat"))
        .json(&json!({
            "model": REQUIRED_MODEL,
            "messages": [
                {
                    "role": "user",
                    "content": "Do not call tools. Say OK."
                }
            ],
            "stream": false,
            "tools": tools,
            "options": {
                "num_predict": 8,
                "temperature": 0
            }
        }))
        .send()
        .await
        .unwrap_or_else(|error| panic!("send Ollama chat request for {label}: {error}"));
    let status = response.status();
    let body = response
        .text()
        .await
        .unwrap_or_else(|error| format!("<failed to read body: {error}>"));
    assert!(
        status.is_success(),
        "Ollama rejected {label}: status {}: {body}",
        status.as_u16()
    );
}
