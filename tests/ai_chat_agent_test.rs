//! L3 integration test for stateful ChatAgent conversation with real Gemini API.
//!
//! Drives a `ChatAgent` (which wraps a base `Agent` plus a turn history) across two
//! prompts to confirm history is forwarded into subsequent completion requests — the
//! canonical test of the runtime's "memory" wiring against a live provider.
//!
//! **Layer:** L3 — opt-in live gate. Skipped unless `LIBRA_AI_LIVE_GEMINI=1`
//! and `GEMINI_API_KEY` are both set so default tests do not depend on Google
//! Cloud project API enablement.

use libra::internal::ai::{
    agent::{AgentBuilder, ChatAgent},
    providers::gemini::Client,
};

/// Return `true` only for an explicit live Gemini run.
///
/// Boundary: a configured `GEMINI_API_KEY` alone is insufficient because `.env.test`
/// may contain a key for a project where the Generative Language API is disabled.
fn live_gemini_enabled() -> bool {
    std::env::var("LIBRA_AI_LIVE_GEMINI").is_ok_and(|value| value == "1")
        && std::env::var("GEMINI_API_KEY").is_ok_and(|value| !value.is_empty())
}

/// Integration test for ChatAgent state management with real Gemini API.
///
/// Scenario: opens a two-turn conversation. Turn 1 establishes the user's name; Turn 2
/// asks for it back. The test passes only when the second response references "libra",
/// proving that the `ChatAgent` actually replays history into each completion request.
/// Also asserts the in-memory transcript is exactly four messages (user/asst x 2),
/// catching regressions where history is dropped or duplicated.
///
/// Boundary: skipped unless `LIBRA_AI_LIVE_GEMINI=1` and `GEMINI_API_KEY` are both
/// set.
///
/// # Setup
/// This test requires a valid `GEMINI_API_KEY` environment variable.
///
/// ```bash
/// export LIBRA_AI_LIVE_GEMINI=1
/// export GEMINI_API_KEY="your_key_here"
/// cargo test --test ai_chat_agent_test test_chat_agent_conversation
/// ```
#[tokio::test]
async fn test_chat_agent_conversation() {
    if !live_gemini_enabled() {
        eprintln!("skipped (set LIBRA_AI_LIVE_GEMINI=1 and GEMINI_API_KEY to run Gemini gate)");
        return;
    }

    // 1. Create Client and Model
    let client = Client::from_env().expect("Failed to create client");
    let model = client.completion_model("gemini-2.5-flash");

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
