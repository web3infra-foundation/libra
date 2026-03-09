//! Codex completion model implementation using WebSocket.

use std::sync::{Arc, RwLock};

use walkdir::WalkDir;

use crate::internal::ai::{
    client::CompletionClient,
    completion::{
        CompletionError, CompletionModel as CompletionModelTrait,
        message::{AssistantContent, Text},
        request::{CompletionRequest, CompletionResponse},
    },
    mcp::server::LibraMcpServer,
    providers::codex::client::CodexWebSocket,
};

pub const CODEX_01: &str = "codex-01";

#[derive(Clone)]
pub struct Model {
    client: CodexWebSocket,
    model: String,
    mcp_server: Option<Arc<LibraMcpServer>>,
    run_id: Arc<RwLock<Option<String>>>,
}

impl std::fmt::Debug for Model {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Model")
            .field("client", &self.client)
            .field("model", &self.model)
            .field("mcp_server", &self.mcp_server.is_some())
            .finish()
    }
}

impl Model {
    pub fn new(
        client: CodexWebSocket,
        model: impl Into<String>,
        mcp_server: Option<Arc<LibraMcpServer>>,
    ) -> Self {
        Self {
            client,
            model: model.into(),
            mcp_server,
            run_id: Arc::new(RwLock::new(None)),
        }
    }

    pub fn model_name(&self) -> &str {
        &self.model
    }

    /// Set the run ID for linking patchsets to the current run.
    pub fn set_run_id(&self, run_id: String) {
        if let Ok(mut guard) = self.run_id.write() {
            *guard = Some(run_id);
        }
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
                        crate::internal::ai::completion::message::OneOrMany::One(c) => match c {
                            crate::internal::ai::completion::message::UserContent::Text(t) => {
                                t.text.clone()
                            }
                            _ => format!("{:?}", c),
                        },
                        crate::internal::ai::completion::message::OneOrMany::Many(vec) => vec
                            .iter()
                            .map(|c| match c {
                                crate::internal::ai::completion::message::UserContent::Text(t) => {
                                    t.text.clone()
                                }
                                _ => format!("{:?}", c),
                            })
                            .collect::<Vec<_>>()
                            .join("\n"),
                    };
                    Some(text)
                } else {
                    None
                }
            })
            .unwrap_or_default();

        // Get the working directory for Codex to use
        let working_dir = crate::utils::util::working_dir();
        let working_dir_str = working_dir.to_string_lossy().to_string();

        // Snapshot current files before Codex runs (recursive to match fallback detection)
        let mut previous_files = std::collections::HashSet::new();
        if working_dir.exists() {
            for entry in WalkDir::new(&working_dir)
                .into_iter()
                .filter_map(|e| e.ok())
            {
                let path = entry.path();
                if path.is_file()
                    && let Ok(rel_path) = path.strip_prefix(&working_dir)
                {
                    let rel_str = rel_path.to_string_lossy().replace("\\", "/");
                    if !rel_str.starts_with(".git") && !rel_str.starts_with(".libra") {
                        previous_files.insert(rel_str);
                    }
                }
            }
        }

        // Set approvalPolicy to "never" to auto-approve all operations (file changes, command execution)
        // Also set cwd to use Libra's working directory
        let params = serde_json::json!({
            "input": [{
                "type": "text",
                "text": user_message
            }],
            "approvalPolicy": "never",
            "cwd": working_dir_str
        });

        let result = self
            .client
            .clone()
            .send_request_with_thread("turn/start", params)
            .await
            .map_err(|e| CompletionError::ProviderError(e.to_string()))?;

        // Extract content and file changes from response
        let (content, mut file_changes) = extract_content_and_changes(&result);

        // If no file changes from response, detect them from file system
        if file_changes.is_empty() {
            let detected_changes = detect_file_changes(&working_dir, &previous_files);
            file_changes = detected_changes;
        }

        // Apply file changes to working directory
        if !file_changes.is_empty() {
            // Apply changes and collect any errors
            let mut errors = Vec::new();
            for change in &file_changes {
                if let Err(e) = apply_file_change(change) {
                    errors.push(format!("{}: {}", change.path, e));
                }
            }

            // Log errors if any
            if !errors.is_empty() {
                tracing::warn!("File apply errors: {}", errors.join("; "));
            }

            // Auto-add and commit changes to Libra via MCP
            if let Err(e) = self.commit_to_libra(&file_changes).await {
                tracing::warn!("Failed to commit to Libra via MCP: {}", e);
            }
        }

        if content.is_empty() {
            // Fallback to streaming message if response extraction failed
            let message = self.client.get_agent_messages().await;
            if !message.is_empty() {
                return Ok(CompletionResponse {
                    content: vec![AssistantContent::Text(Text {
                        text: message.join(
                            "
",
                        ),
                    })],
                    raw_response: result,
                });
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
    operation: String,       // "add", "update", "delete"
    content: Option<String>, // Full content for new files
}

/// Detect file changes in the working directory by comparing with a previous snapshot
fn detect_file_changes(
    working_dir: &std::path::Path,
    previous_files: &std::collections::HashSet<String>,
) -> Vec<FileChange> {
    let mut file_changes = Vec::new();

    if !working_dir.exists() {
        return file_changes;
    }

    // Get current files in working directory
    let mut current_files = std::collections::HashSet::new();

    // Walk through all files in the working directory recursively
    for entry in WalkDir::new(working_dir).into_iter().filter_map(|e| e.ok()) {
        let path = entry.path();
        if path.is_file() {
            // Get relative path
            if let Ok(rel_path) = path.strip_prefix(working_dir) {
                let rel_str = rel_path.to_string_lossy().replace("\\", "/");
                // Skip .git and .libra directories
                if rel_str.starts_with(".git") || rel_str.starts_with(".libra") {
                    continue;
                }

                current_files.insert(rel_str.clone());

                // Check if this is a new file (not in previous)
                if !previous_files.contains(&rel_str) {
                    // New file - read the content
                    if let Ok(content) = std::fs::read_to_string(path) {
                        file_changes.push(FileChange {
                            path: rel_str,
                            diff: String::new(),
                            operation: "add".to_string(),
                            content: Some(content),
                        });
                    }
                } else {
                    // Existing file - check if content changed
                    // Note: For simplicity, we treat all existing files as potentially modified
                    // In production, you'd want to compare actual content hashes
                    if let Ok(_content) = std::fs::read_to_string(path) {
                        // For now, we don't mark as modified since we can't easily compare
                        // The main detection is for new files from Codex
                    }
                }
            }
        }
    }

    // Detect deleted files (were in previous but not in current)
    for prev_file in previous_files {
        if !current_files.contains(prev_file) {
            // Skip .git and .libra
            if prev_file.starts_with(".git") || prev_file.starts_with(".libra") {
                continue;
            }
            file_changes.push(FileChange {
                path: prev_file.clone(),
                diff: String::new(),
                operation: "delete".to_string(),
                content: None,
            });
        }
    }

    file_changes
}

fn extract_content_and_changes(
    response: &serde_json::Value,
) -> (Vec<AssistantContent>, Vec<FileChange>) {
    let mut content = Vec::new();
    let mut file_changes = Vec::new();

    // Try result.turn first
    if let Some(turn) = response.get("result").and_then(|r| r.get("turn")) {
        extract_from_turn(turn, &mut content, &mut file_changes);
    }

    // If not found, try result.thread.turns (from thread/read API)
    if content.is_empty()
        && file_changes.is_empty()
        && let Some(thread) = response.get("result").and_then(|r| r.get("thread"))
        && let Some(turns) = thread.get("turns").and_then(|t| t.as_array())
        && let Some(last_turn) = turns.last()
    {
        extract_from_turn(last_turn, &mut content, &mut file_changes);
    }

    // Don't inject placeholder - let caller handle empty content via fallback
    // (e.g., get_agent_messages for streamed output)

    (content, file_changes)
}

fn extract_from_turn(
    turn: &serde_json::Value,
    content: &mut Vec<AssistantContent>,
    file_changes: &mut Vec<FileChange>,
) {
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
                                let path = change
                                    .get("path")
                                    .and_then(|p| p.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                let diff = change
                                    .get("diff")
                                    .and_then(|d| d.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                // Try to get content for new files
                                let content_field = change
                                    .get("content")
                                    .and_then(|c| c.as_str())
                                    .map(String::from);
                                let operation = if let Some(kind) = change.get("kind") {
                                    if let Some(kind_type) =
                                        kind.get("type").and_then(|t| t.as_str())
                                    {
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

    // Security check: validate the path stays within working directory
    // This prevents path traversal attacks (e.g., ../../../etc/passwd)
    let canonical_working_dir = working_dir
        .canonicalize()
        .map_err(|e| format!("failed to canonicalize working directory: {}", e))?;

    // For non-existent files, we need to check if the parent directory is safe
    // Or if the path contains parent directory references (..)
    let normalized_path = change.path.replace("\\", "/");

    // Reject paths that try to escape the working directory
    if normalized_path.contains("..") {
        return Err(format!(
            "security: file path '{}' contains parent directory references (not allowed)",
            change.path
        )
        .into());
    }

    // Additional check for absolute paths
    if std::path::Path::new(&change.path).is_absolute() {
        return Err(format!(
            "security: file path '{}' is absolute (not allowed)",
            change.path
        )
        .into());
    }

    // Verify the final path is within working directory
    let canonical_file_path = file_path.canonicalize().unwrap_or_else(|_| {
        // If file doesn't exist, check if parent directory is safe
        // by canonicalizing the parent
        file_path
            .parent()
            .map(|p| p.canonicalize().unwrap_or_else(|_| p.to_path_buf()))
            .unwrap_or_else(|| file_path.to_path_buf())
    });

    // Use Path::starts_with for component-aware path validation (not string prefix)
    // This prevents bypasses like /workspace/libra2/ vs /workspace/libra/
    if !canonical_file_path.starts_with(&canonical_working_dir)
        && canonical_file_path != canonical_working_dir
    {
        return Err(format!(
            "security: file path '{}' escapes working directory '{}'",
            change.path,
            canonical_working_dir.display()
        )
        .into());
    }

    match change.operation.as_str() {
        "delete" => {
            if file_path.exists() {
                std::fs::remove_file(&file_path)?;
                // eprintln!("[Codex] Deleted file: {}", change.path);
            }
        }
        "add" | "update" => {
            // First try content field for new files
            if change.operation == "add"
                && let Some(ref file_content) = change.content
                && let Some(parent) = file_path.parent()
            {
                std::fs::create_dir_all(parent)?;
                std::fs::write(&file_path, file_content)?;
                // eprintln!("[Codex] Wrote new file from content: {}", change.path);
                return Ok(());
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
                            // eprintln!("[Codex] Applied diff to: {}", change.path);
                        }
                        Err(e) => {
                            // If diff application fails, write the entire content if it's a new file
                            // For existing files, propagate the error to avoid silent failures
                            if !file_path.exists() {
                                if let Some(parent) = file_path.parent() {
                                    std::fs::create_dir_all(parent)?;
                                }
                                // For add operation, the diff might contain the full content
                                std::fs::write(&file_path, &change.diff)?;
                            } else {
                                return Err(format!(
                                    "failed to apply diff to existing file '{}': {}",
                                    change.path, e
                                )
                                .into());
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
                        // eprintln!("[Codex] Wrote new file: {}", change.path);
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
        Model::new(self.clone(), model, None)
    }
}

pub type CodexModel = Model;

impl Model {
    /// Commit file changes to Libra via MCP tools
    /// Commit file changes to Libra via MCP tools
    async fn commit_to_libra(
        &self,
        file_changes: &[FileChange],
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        use git_internal::internal::object::types::ActorRef;

        use crate::internal::ai::mcp::resource::{
            ArtifactParams, CreatePatchSetParams, TouchedFileParams,
        };

        // Get list of changed files
        let files: Vec<String> = file_changes.iter().map(|c| c.path.clone()).collect();
        if files.is_empty() {
            return Ok(());
        }

        // Convert file changes to TouchedFileParams
        let touched_files: Vec<TouchedFileParams> = file_changes
            .iter()
            .map(|c| {
                let change_type = match c.operation.as_str() {
                    "create" => "add",
                    "add" => "add",
                    "write" | "update" => "modify",
                    "delete" => "delete",
                    _ => "modify",
                };
                let lines = c
                    .content
                    .as_ref()
                    .map(|s| s.lines().count() as u32)
                    .unwrap_or(0);
                TouchedFileParams {
                    path: c.path.clone(),
                    change_type: change_type.to_string(),
                    lines_added: lines,
                    lines_deleted: 0,
                }
            })
            .collect();

        // Build diff artifact from file changes (concatenate all file contents)
        let diff_content: String = file_changes
            .iter()
            .filter_map(|c| c.content.as_ref())
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join("\n");

        let diff_artifact = if !diff_content.is_empty() {
            Some(ArtifactParams {
                store: "memory".to_string(),
                key: format!("codex-diff-{}", uuid::Uuid::new_v4()),
                content_type: Some("text/plain".to_string()),
                size_bytes: Some(diff_content.len() as u64),
                hash: None,
            })
        } else {
            None
        };

        // Use placeholder for base commit (Libra uses SHA-256 internally)
        // "0" * 64 = "0000...000" is accepted by normalize_commit_anchor
        let base_commit = "0".repeat(64);

        // Create ActorRef for Codex
        let actor =
            ActorRef::agent("codex").map_err(|e| format!("failed to create actor: {}", e))?;

        // Create PatchSet via MCP
        if let Some(mcp_server) = &self.mcp_server {
            let run_id = self
                .run_id
                .read()
                .ok()
                .and_then(|guard| guard.clone())
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
            let params = CreatePatchSetParams {
                run_id,
                generation: 1,
                sequence: None,
                base_commit_sha: base_commit,
                touched_files: Some(touched_files),
                rationale: Some(format!("Codex generated files: {}", files.join(", "))),
                diff_format: Some("unified_diff".to_string()),
                diff_artifact,
                tags: None,
                external_ids: None,
                actor_kind: Some("agent".to_string()),
                actor_id: Some("codex".to_string()),
            };

            // Call the MCP tool directly via create_patchset_impl
            match mcp_server.create_patchset_impl(params, actor).await {
                Ok(_result) => {
                    // tracing::info!("Created patchset via MCP: {:?}", _result);
                }
                Err(_e) => {
                    // tracing::warn!("Failed to create patchset via MCP: {:?}", _e);
                }
            }
        }

        Ok(())
    }
}
