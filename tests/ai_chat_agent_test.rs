//! L3 integration test for stateful ChatAgent conversation with the live DeepSeek API.
//!
//! Drives a `ChatAgent` (which wraps a base `Agent` plus a turn history) across two
//! prompts to confirm history is forwarded into subsequent completion requests — the
//! canonical test of the runtime's "memory" wiring against a live provider.
//!
//! **Layer:** L3 — gated on `DEEPSEEK_API_KEY` being set in the environment. The
//! key alone is sufficient because DeepSeek's API enablement is part of the
//! account; there's no parallel project-API-enablement gate to worry about
//! (unlike the previous Gemini setup, which needed a separate
//! `LIBRA_AI_LIVE_GEMINI=1` opt-in to avoid running against a project where
//! the Generative Language API was disabled). With `source .env.test` exporting
//! `DEEPSEEK_API_KEY`, this test runs and is expected to pass.

use libra::internal::ai::{
    agent::{AgentBuilder, ChatAgent},
    client::CompletionClient,
    providers::deepseek::Client,
};

/// L3 model used by every DeepSeek-backed integration test in this crate.
///
/// `deepseek-v4-flash` is the cheap / fast tier that matches what
/// `.env.test`-backed local runs and the GitHub Actions L3 nightly job pay
/// for. Tests must not silently switch to a costlier tier (e.g.
/// `deepseek-v4-pro`) without a deliberate per-test override.
const DEEPSEEK_TEST_MODEL: &str = "deepseek-v4-flash";

/// Return `true` when the DeepSeek live gate is satisfied — i.e. a non-empty
/// `DEEPSEEK_API_KEY` is in the process environment. Sourcing `.env.test`
/// before `cargo test --all` is the documented way to enable L3 AI tests.
fn deepseek_live_enabled() -> bool {
    std::env::var("DEEPSEEK_API_KEY").is_ok_and(|value| !value.trim().is_empty())
}

/// Integration test for ChatAgent state management with the live DeepSeek API.
///
/// Scenario: opens a two-turn conversation. Turn 1 establishes the user's name; Turn 2
/// asks for it back. The test passes only when the second response references "libra",
/// proving that the `ChatAgent` actually replays history into each completion request.
/// Also asserts the in-memory transcript is exactly four messages (user/asst x 2),
/// catching regressions where history is dropped or duplicated.
///
/// Boundary: skipped when `DEEPSEEK_API_KEY` is unset. When the key is set the
/// test reaches DeepSeek's `/v1/chat/completions` endpoint and is expected to
/// pass — failure here represents a real regression in the provider client or
/// the chat runtime.
///
/// # Setup
///
/// ```bash
/// source .env.test && cargo test --test ai_chat_agent_test test_chat_agent_conversation
/// ```
///
/// or set the key inline:
///
/// ```bash
/// DEEPSEEK_API_KEY=sk-... cargo test --test ai_chat_agent_test test_chat_agent_conversation
/// ```
#[tokio::test]
async fn test_chat_agent_conversation() {
    if !deepseek_live_enabled() {
        eprintln!("skipped (set DEEPSEEK_API_KEY to run the DeepSeek L3 gate)");
        return;
    }

    // 1. Create Client and Model
    let client = Client::from_env().expect("DEEPSEEK_API_KEY missing despite gate");
    let model = client.completion_model(DEEPSEEK_TEST_MODEL);

    // 2. Create Base Agent
    let agent = AgentBuilder::new(model)
        .preamble("You are a helpful assistant. Keep answers very short.")
        .temperature(0.0)
        .expect("Invalid temperature")
        .build();

    // 3. Create ChatAgent (Stateful)
    let mut chat_agent = ChatAgent::new(agent);

    // 4. Turn 1: Set context
    println!("Sending turn 1...");
    let resp1 = chat_agent.chat("My name is Libra.").await;
    assert!(resp1.is_ok(), "Turn 1 failed: {:?}", resp1.err());
    let content1 = resp1.unwrap();
    println!("Turn 1 response: {}", content1);

    // 5. Turn 2: Verify context retention
    println!("Sending turn 2...");
    let resp2 = chat_agent.chat("What is my name?").await;
    assert!(resp2.is_ok(), "Turn 2 failed: {:?}", resp2.err());
    let content2 = resp2.unwrap();
    println!("Turn 2 response: {}", content2);

    // Verify the response contains the name "Libra"
    assert!(
        content2.to_lowercase().contains("libra"),
        "ChatAgent failed to remember the name. Response was: {}",
        content2
    );

    // 6. Verify History Length
    // History should have 4 messages: User1, Asst1, User2, Asst2
    assert_eq!(chat_agent.history().len(), 4);
}
