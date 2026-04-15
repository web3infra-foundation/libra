//! Phase E hardening contracts for authorization, tool boundary, redaction, and audit.

use std::collections::BTreeSet;

use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use uuid::Uuid;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrincipalRole {
    Owner,
    Contributor,
    Observer,
    System,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrincipalContext {
    pub principal_id: String,
    pub role: PrincipalRole,
}

impl PrincipalContext {
    pub fn system() -> Self {
        Self {
            principal_id: "libra-runtime".to_string(),
            role: PrincipalRole::System,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolOperation {
    pub tool_name: String,
    pub mutates_state: bool,
    pub requires_network: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoundaryDecision {
    pub allowed: bool,
    pub approval_required: bool,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolBoundaryPolicy {
    readonly_tools: BTreeSet<String>,
    mutating_tools: BTreeSet<String>,
    allow_network: bool,
    policy_version: String,
}

impl ToolBoundaryPolicy {
    pub fn default_runtime() -> Self {
        Self {
            readonly_tools: ["read_file", "list_dir", "grep_files", "mcp_read"]
                .into_iter()
                .map(str::to_string)
                .collect(),
            mutating_tools: ["shell", "apply_patch", "mcp_write"]
                .into_iter()
                .map(str::to_string)
                .collect(),
            allow_network: false,
            policy_version: "tool-boundary:v1".to_string(),
        }
    }

    pub fn policy_version(&self) -> &str {
        &self.policy_version
    }

    pub fn decide(
        &self,
        principal: &PrincipalContext,
        operation: &ToolOperation,
    ) -> BoundaryDecision {
        if operation.requires_network && !self.allow_network {
            return BoundaryDecision {
                allowed: false,
                approval_required: false,
                reason: "network access is disabled by tool boundary policy".to_string(),
            };
        }

        if principal.role == PrincipalRole::Observer && operation.mutates_state {
            return BoundaryDecision {
                allowed: false,
                approval_required: false,
                reason: "observer principals cannot run mutating tools".to_string(),
            };
        }

        if self.readonly_tools.contains(&operation.tool_name) && !operation.mutates_state {
            return BoundaryDecision {
                allowed: true,
                approval_required: false,
                reason: "readonly tool allowed".to_string(),
            };
        }

        if self.mutating_tools.contains(&operation.tool_name) || operation.mutates_state {
            return BoundaryDecision {
                allowed: true,
                approval_required: principal.role != PrincipalRole::System,
                reason: "mutating tool requires runtime-mediated approval".to_string(),
            };
        }

        BoundaryDecision {
            allowed: false,
            approval_required: false,
            reason: format!("unknown tool '{}' is not allowlisted", operation.tool_name),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SecretRedactor {
    markers: Vec<String>,
}

impl SecretRedactor {
    pub fn default_runtime() -> Self {
        Self {
            markers: [
                "api_key:",
                "api_key=",
                "authorization: bearer ",
                "password:",
                "password=",
                "token:",
                "token=",
            ]
            .into_iter()
            .map(str::to_string)
            .collect(),
        }
    }

    pub fn redact(&self, input: &str) -> String {
        let mut output = input.to_string();
        for marker in &self.markers {
            output = redact_marker(&output, marker);
        }
        output
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditEvent {
    pub trace_id: Uuid,
    pub principal_id: String,
    pub action: String,
    pub policy_version: String,
    pub redacted_summary: String,
    pub at: DateTime<Utc>,
}

#[async_trait]
pub trait AuditSink: Send + Sync {
    async fn append(&self, event: AuditEvent) -> Result<()>;
    async fn flush(&self) -> Result<()>;
}

#[derive(Debug, Default)]
pub struct InMemoryAuditSink {
    events: Mutex<Vec<AuditEvent>>,
}

impl InMemoryAuditSink {
    pub async fn events(&self) -> Vec<AuditEvent> {
        self.events.lock().await.clone()
    }
}

#[async_trait]
impl AuditSink for InMemoryAuditSink {
    async fn append(&self, event: AuditEvent) -> Result<()> {
        self.events.lock().await.push(event);
        Ok(())
    }

    async fn flush(&self) -> Result<()> {
        Ok(())
    }
}

fn redact_marker(input: &str, marker: &str) -> String {
    let lower = input.to_lowercase();
    let mut cursor = 0;
    let mut output = String::with_capacity(input.len());

    while let Some(relative_start) = lower[cursor..].find(marker) {
        let marker_start = cursor + relative_start;
        let value_start = marker_start + marker.len();
        output.push_str(&input[cursor..value_start]);

        let mut value_cursor = value_start;
        while let Some(ch) = input[value_cursor..].chars().next() {
            if !ch.is_whitespace() {
                break;
            }
            output.push(ch);
            value_cursor += ch.len_utf8();
        }

        let value_end = input[value_cursor..]
            .char_indices()
            .find_map(|(offset, ch)| {
                if ch.is_whitespace() || ch == ',' || ch == ';' {
                    Some(value_cursor + offset)
                } else {
                    None
                }
            })
            .unwrap_or(input.len());

        output.push_str("[REDACTED]");
        cursor = value_end;
    }

    output.push_str(&input[cursor..]);
    output
}
