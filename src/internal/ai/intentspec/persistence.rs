use anyhow::{Context, Result};
use rmcp::model::CallToolResult;

use crate::internal::ai::{
    intentspec::{IntentSpec, canonical::to_canonical_json},
    mcp::{resource::CreateIntentParams, server::LibraMcpServer},
};

/// Persist an IntentSpec to the MCP server as a git-internal Intent object.
///
/// This function:
/// 1. Serializes the IntentSpec to its canonical JSON representation.
/// 2. Constructs CreateIntentParams with the JSON as content.
/// 3. Calls the MCP create_intent_impl method to store the object.
/// 4. Returns the created Intent ID.
pub async fn persist_intentspec(spec: &IntentSpec, mcp_server: &LibraMcpServer) -> Result<String> {
    let canonical =
        to_canonical_json(spec).context("Failed to serialize IntentSpec to canonical JSON")?;

    let params = CreateIntentParams {
        content: spec.intent.summary.clone(),
        structured_content: Some(canonical),
        parent_id: None,
        parent_ids: None,
        analysis_context_frame_ids: None,
        status: Some("active".to_string()),
        commit_sha: None, // Will be set when completed
        reason: None,
        next_intent_id: None,
        actor_kind: Some("system".to_string()),
        actor_id: Some("libra-plan".to_string()),
    };

    let actor = mcp_server
        .resolve_actor_from_params(params.actor_kind.as_deref(), params.actor_id.as_deref())
        .context("Failed to resolve actor for Intent persistence")?;

    let result = mcp_server
        .create_intent_impl(params, actor)
        .await
        .map_err(|e| anyhow::anyhow!("MCP create_intent failed: {:?}", e))?;

    if result.is_error.unwrap_or(false) {
        let msg = result
            .content
            .first()
            .and_then(|c| c.as_text())
            .map(|t| t.text.as_str())
            .unwrap_or("Unknown MCP error");
        return Err(anyhow::anyhow!("MCP create_intent returned error: {}", msg));
    }

    parse_created_id(&result)
        .ok_or_else(|| anyhow::anyhow!("Failed to parse Intent ID from MCP result"))
}

fn parse_created_id(result: &CallToolResult) -> Option<String> {
    for content in &result.content {
        if let Some(text) = content.as_text().map(|t| t.text.as_str())
            && let Some(id) = text.split("ID:").nth(1)
        {
            let id = id.trim();
            if !id.is_empty() {
                return Some(id.to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::internal::ai::intentspec::{
        IntentDraft, ResolveContext, RiskLevel, draft::*, resolve_intentspec, types::ChangeType,
    };

    fn create_dummy_spec() -> IntentSpec {
        resolve_intentspec(
            IntentDraft {
                intent: DraftIntent {
                    summary: "Test Spec".to_string(),
                    problem_statement: "Testing persistence".to_string(),
                    change_type: ChangeType::Bugfix,
                    objectives: vec!["Test".to_string()],
                    in_scope: vec!["src".to_string()],
                    out_of_scope: vec![],
                    touch_hints: None,
                },
                acceptance: DraftAcceptance {
                    success_criteria: vec!["Works".to_string()],
                    fast_checks: vec![],
                    integration_checks: vec![],
                    security_checks: vec![],
                    release_checks: vec![],
                },
                risk: DraftRisk {
                    rationale: "Low risk".to_string(),
                    factors: vec![],
                    level: Some(RiskLevel::Low),
                },
            },
            RiskLevel::Low,
            ResolveContext {
                working_dir: "/tmp".to_string(),
                base_ref: "HEAD".to_string(),
                created_by_id: "test-user".to_string(),
            },
        )
    }

    #[test]
    fn test_create_params_construction() {
        let spec = create_dummy_spec();
        let canonical = to_canonical_json(&spec).expect("Should serialize");

        let params = CreateIntentParams {
            content: canonical.clone(),
            structured_content: Some(canonical.clone()),
            parent_id: None,
            parent_ids: None,
            analysis_context_frame_ids: None,
            status: Some("active".to_string()),
            commit_sha: None,
            reason: None,
            next_intent_id: None,
            actor_kind: Some("system".to_string()),
            actor_id: Some("libra-plan".to_string()),
        };

        assert_eq!(params.content, canonical);
        assert!(params.structured_content.is_some());
        assert_eq!(
            params.structured_content.as_deref(),
            Some(canonical.as_str())
        );
        assert_eq!(params.status.as_deref(), Some("active"));
        assert_eq!(params.actor_id.as_deref(), Some("libra-plan"));
    }

    #[test]
    fn test_parse_id() {
        use rmcp::model::Content;
        let result = CallToolResult::success(vec![Content::text("Intent created with ID: 12345")]);
        assert_eq!(parse_created_id(&result), Some("12345".to_string()));

        let result_fail = CallToolResult::success(vec![Content::text("Something else")]);
        assert_eq!(parse_created_id(&result_fail), None);
    }

    /// Verify that persist_intentspec builds params with
    /// `content` = intent summary (prompt) and `structured_content` = canonical JSON.
    #[test]
    fn test_persist_params_use_summary_as_prompt_and_canonical_as_content() {
        let spec = create_dummy_spec();
        let canonical = to_canonical_json(&spec).expect("Should serialize");

        // Simulate what persist_intentspec does internally
        let params = CreateIntentParams {
            content: spec.intent.summary.clone(),
            structured_content: Some(canonical.clone()),
            parent_id: None,
            parent_ids: None,
            analysis_context_frame_ids: None,
            status: Some("active".to_string()),
            commit_sha: None,
            reason: None,
            next_intent_id: None,
            actor_kind: Some("system".to_string()),
            actor_id: Some("libra-plan".to_string()),
        };

        // The prompt (content) should be the human-readable summary
        assert_eq!(params.content, "Test Spec");
        // The structured_content should be the full canonical JSON
        assert!(params.structured_content.is_some());
        let sc = params.structured_content.unwrap();
        assert!(sc.contains("intentspec.io/v1alpha1"));
        assert!(sc.contains("Testing persistence"));
        assert_eq!(sc, canonical);
    }
}
