use std::env;

use libra::internal::ai::{
    agent::{AgentBuilder, ChatAgent},
    providers::gemini::Client,
};

/// Integration test for ChatAgent state management with real Gemini API.
///
/// # Setup
/// This test requires a valid `GEMINI_API_KEY` environment variable.
///
/// ```bash
/// export GEMINI_API_KEY="your_key_here"
/// cargo test test_chat_agent_conversation
/// ```
#[tokio::test]
async fn test_chat_agent_conversation() {
    // Check for API Key
    if env::var("GEMINI_API_KEY").is_err() {
        println!("Skipping test_chat_agent_conversation because GEMINI_API_KEY is not set.");
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
        content2.contains("Libra"),
        "ChatAgent failed to remember the name. Response was: {}",
        content2
    );

    // 6. Verify History Length
    // History should have 4 messages: User1, Asst1, User2, Asst2
    assert_eq!(chat_agent.history().len(), 4);
}
