//! Local-only Phase 0 live gate for Ollama.
//!
//! Run manually with:
//! `LIBRA_AI_LIVE_OLLAMA=1 OLLAMA_HOST=http://127.0.0.1:11434 cargo test --test ai_ollama_live_gate_test`

use std::time::Duration;

use serde::Deserialize;
use serde_json::json;

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
