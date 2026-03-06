//! Codex completion model implementation using WebSocket.


use crate::internal::ai::{
    client::CompletionClient,
    completion::{
        message::{AssistantContent, Text},
        CompletionError, CompletionModel as CompletionModelTrait,
        request::{CompletionRequest, CompletionResponse},
    },
    providers::codex::client::CodexWebSocket,
};

pub const CODEX_01: &str = "codex-01";

#[derive(Clone, Debug)]
pub struct Model {
    client: CodexWebSocket,
    model: String,
}

impl Model {
    pub fn new(client: CodexWebSocket, model: impl Into<String>) -> Self {
        Self {
            client,
            model: model.into(),
        }
    }

    pub fn model_name(&self) -> &str {
        &self.model
    }

    pub async fn connect(&mut self) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        self.client.connect().await
    }
}


impl CompletionModelTrait for Model {
    type Response = serde_json::Value;

    async fn completion(
        &self,
        request: CompletionRequest,
    ) -> Result<CompletionResponse<Self::Response>, CompletionError> {
        let user_message = request
            .chat_history
            .last()
            .and_then(|msg| {
                use crate::internal::ai::completion::message::Message;
                if let Message::User { content } = msg {
                    let text = match content {
                        crate::internal::ai::completion::message::OneOrMany::One(c) => {
                            match c {
                                crate::internal::ai::completion::message::UserContent::Text(t) => t.text.clone(),
                                _ => format!("{:?}", c),
                            }
                        }
                        crate::internal::ai::completion::message::OneOrMany::Many(vec) => {
                            vec.iter()
                                .map(|c| match c {
                                    crate::internal::ai::completion::message::UserContent::Text(t) => t.text.clone(),
                                    _ => format!("{:?}", c),
                                })
                                .collect::<Vec<_>>()
                                .join("\n")
                        }
                    };
                    Some(text)
                } else {
                    None
                }
            })
            .unwrap_or_default();

        // Set approvalPolicy to "never" to auto-approve all operations (file changes, command execution)
        let params = serde_json::json!({
            "input": [{
                "type": "text",
                "text": user_message
            }],
            "approvalPolicy": "never"
        });

        let result = self
            .client
            .clone()
            .send_request_with_thread("turn/start", params)
            .await
            .map_err(|e| CompletionError::ProviderError(e.to_string()))?;

        // Extract content and file changes from response
        let (content, file_changes) = extract_content_and_changes(&result);

        // Apply file changes to working directory
        if !file_changes.is_empty() {
            eprintln!("[Codex] Applying {} file changes...", file_changes.len());
            for change in &file_changes {
                eprintln!("[Codex] - {}: {}", change.operation, change.path);
                if let Err(e) = apply_file_change(change) {
                    eprintln!("[Codex] Failed to apply change to {}: {}", change.path, e);
                }
            }

            // Auto-add and commit changes to Libra
            eprintln!("[Codex] Committing changes to Libra...");
            if let Err(e) = commit_to_libra(&file_changes).await {
                eprintln!("[Codex] Failed to commit: {}", e);
            }
        }

        if content.is_empty() {
            // Fallback to streaming message if response extraction failed
            let message = self.client.get_agent_messages().await;
            eprintln!("[Codex] Using fallback message, len: {}", message.len());
            if !message.is_empty() {
                return Ok(CompletionResponse {
                    content: vec![AssistantContent::Text(Text { text: message.join("
") })],
                    raw_response: result,
                });
            }
        } else {
            eprintln!("[Codex] Extracted content from response, {} items", content.len());
            for (i, c) in content.iter().enumerate() {
                eprintln!("[Codex] Content {}: {:?}", i, c);
            }
        };

        self.client.clear_agent_messages().await;

        Ok(CompletionResponse {
            content,
            raw_response: result,
        })
    }
}

/// Represents a file change from Codex
#[derive(Debug, Clone)]
struct FileChange {
    path: String,
    diff: String,
    operation: String, // "add", "update", "delete"
    content: Option<String>, // Full content for new files
}

fn extract_content_and_changes(response: &serde_json::Value) -> (Vec<AssistantContent>, Vec<FileChange>) {
    let mut content = Vec::new();
    let mut file_changes = Vec::new();

    // Debug: print the full response
    let resp_str = serde_json::to_string(&response).unwrap_or_default();
    eprintln!("[Codex] extract_content_and_changes, response len: {}", resp_str.len());
    eprintln!("[Codex] response preview: {}", resp_str.chars().take(200).collect::<String>());
    // Also check for error in status
    if let Some(status) = response.get("result").and_then(|r| r.get("thread")).and_then(|t| t.get("status")) {
        eprintln!("[Codex] thread status: {:?}", status);
    // Check for error details
    if let Some(error) = response.get("result").and_then(|r| r.get("error")) {
        eprintln!("[Codex] error in response: {:?}", error);
    }
    }

    // Try result.turn first
    if let Some(turn) = response.get("result").and_then(|r| r.get("turn")) {
        extract_from_turn(turn, &mut content, &mut file_changes);
    }

    // If not found, try result.thread.turns (from thread/read API)
    if content.is_empty() && file_changes.is_empty() {
        if let Some(thread) = response.get("result").and_then(|r| r.get("thread")) {
            if let Some(turns) = thread.get("turns").and_then(|t| t.as_array()) {
                // Get the last turn
                if let Some(last_turn) = turns.last() {
                    extract_from_turn(last_turn, &mut content, &mut file_changes);
                }
            }
        }
    }

    if content.is_empty() {
        content.push(AssistantContent::Text(Text {
            text: "Codex is processing your request...".to_string(),
        }));
    }

    (content, file_changes)
}

fn extract_from_turn(turn: &serde_json::Value, content: &mut Vec<AssistantContent>, file_changes: &mut Vec<FileChange>) {
    if let Some(items) = turn.get("items").and_then(|i| i.as_array()) {
        for item in items {
            if let Some(item_type) = item.get("type").and_then(|t| t.as_str()) {
                match item_type {
                    "agentMessage" => {
                        if let Some(text) = item.get("text").and_then(|t| t.as_str()) {
                            content.push(AssistantContent::Text(Text {
                                text: text.to_string(),
                            }));
                        }
                    }
                    "fileChange" => {
                        if let Some(changes) = item.get("changes").and_then(|c| c.as_array()) {
                            for change in changes {
                                let path = change.get("path").and_then(|p| p.as_str()).unwrap_or("").to_string();
                                let diff = change.get("diff").and_then(|d| d.as_str()).unwrap_or("").to_string();
                                // Try to get content for new files
                                let content_field = change.get("content").and_then(|c| c.as_str()).map(String::from);
                                let operation = if let Some(kind) = change.get("kind") {
                                    if let Some(kind_type) = kind.get("type").and_then(|t| t.as_str()) {
                                        kind_type.to_string()
                                    } else {
                                        "update".to_string()
                                    }
                                } else {
                                    "update".to_string()
                                };

                                if !path.is_empty() {
                                    file_changes.push(FileChange {
                                        path,
                                        diff,
                                        operation,
                                        content: content_field,
                                    });
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}

fn apply_file_change(change: &FileChange) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let working_dir = crate::utils::util::working_dir();
    let file_path = working_dir.join(&change.path);

    match change.operation.as_str() {
        "delete" => {
            if file_path.exists() {
                std::fs::remove_file(&file_path)?;
                eprintln!("[Codex] Deleted file: {}", change.path);
            }
        }
        "add" | "update" => {
            // First try content field for new files
            if change.operation == "add" {
                if let Some(ref file_content) = change.content {
                    if let Some(parent) = file_path.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    std::fs::write(&file_path, file_content)?;
                    eprintln!("[Codex] Wrote new file from content: {}", change.path);
                    return Ok(());
                }
            }

            // Try to apply the diff using diffy
            if !change.diff.is_empty() {
                if let Ok(patch) = diffy::Patch::from_str(&change.diff) {
                    // Read current content if file exists
                    let current_content = if file_path.exists() {
                        std::fs::read_to_string(&file_path).unwrap_or_default()
                    } else {
                        String::new()
                    };

                    // Apply patch
                    match diffy::apply(&current_content, &patch) {
                        Ok(new_content) => {
                            // Ensure parent directory exists
                            if let Some(parent) = file_path.parent() {
                                std::fs::create_dir_all(parent)?;
                            }
                            std::fs::write(&file_path, &new_content)?;
                            eprintln!("[Codex] Applied diff to: {}", change.path);
                        }
                        Err(e) => {
                            // If diff application fails, write the entire content if it's a new file
                            if !file_path.exists() {
                                if let Some(parent) = file_path.parent() {
                                    std::fs::create_dir_all(parent)?;
                                }
                                // For add operation, the diff might contain the full content
                                std::fs::write(&file_path, &change.diff)?;
                                eprintln!("[Codex] Wrote new file: {}", change.path);
                            } else {
                                eprintln!("[Codex] Failed to apply diff to {}: {}", change.path, e);
                            }
                        }
                    }
                } else {
                    // If not a valid patch, try to treat as direct content
                    if !file_path.exists() {
                        if let Some(parent) = file_path.parent() {
                            std::fs::create_dir_all(parent)?;
                        }
                        std::fs::write(&file_path, &change.diff)?;
                        eprintln!("[Codex] Wrote new file: {}", change.path);
                    }
                }
            }
        }
        _ => {}
    }

    Ok(())
}

impl CompletionClient for CodexWebSocket {
    type Model = Model;

    fn completion_model(&self, model: impl Into<String>) -> Self::Model {
        Model::new(self.clone(), model)
    }
}

pub type CodexModel = Model;

async fn commit_to_libra(file_changes: &[FileChange]) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use std::process::Command;

    // Get list of changed files
    let files: Vec<String> = file_changes.iter().map(|c| c.path.clone()).collect();
    if files.is_empty() {
        return Ok(());
    }

    // Run `libra add` for all changed files
    let add_result = Command::new("libra")
        .args(["add", "-A"])
        .output();

    match add_result {
        Ok(output) => {
            if output.status.success() {
                eprintln!("[Codex] Added files to Libra");
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr);
                eprintln!("[Codex] Add failed: {}", stderr);
            }
        }
        Err(e) => {
            eprintln!("[Codex] Failed to run add: {}", e);
        }
    }

    // Run `libra commit` with a message
    let message = format!("Codex: {}", files.join(", "));
    let commit_result = Command::new("libra")
        .args(["commit", "-m", &message])
        .output();

    match commit_result {
        Ok(output) => {
            if output.status.success() {
                let stdout = String::from_utf8_lossy(&output.stdout);
                eprintln!("[Codex] Committed to Libra: {}", stdout.trim());
            } else {
                let stderr = String::from_utf8_lossy(&output.stderr);
                eprintln!("[Codex] Commit failed: {}", stderr);
            }
        }
        Err(e) => {
            eprintln!("[Codex] Failed to run commit: {}", e);
        }
    }

    Ok(())
}
