use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde_json::Value;

use super::{
    super::adapter::{AgentKind, TranscriptTruncator},
    stable_promoted::StablePromotedAgent,
};

impl TranscriptTruncator for StablePromotedAgent {
    fn truncate_transcript(&self, transcript_data: &[u8], checkpoint_id: &str) -> Result<Vec<u8>> {
        let boundary: DateTime<Utc> = checkpoint_id.parse().with_context(|| {
            format!(
                "checkpoint boundary '{checkpoint_id}' must be an RFC-3339 timestamp \
                 (caller is responsible for resolving agent_checkpoint.created_at)"
            )
        })?;
        match self.0.kind {
            AgentKind::Cursor | AgentKind::Codex | AgentKind::Copilot => {
                Ok(truncate_jsonl_timestamp_after(transcript_data, boundary))
            }
            AgentKind::OpenCode => truncate_opencode_messages_after(transcript_data, boundary),
            AgentKind::FactoryAi | AgentKind::ClaudeCode | AgentKind::Gemini => {
                Ok(transcript_data.to_vec())
            }
        }
    }
}

fn truncate_jsonl_timestamp_after(transcript_data: &[u8], boundary: DateTime<Utc>) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::with_capacity(transcript_data.len());
    for line in transcript_data.split_inclusive(|&b| b == b'\n') {
        let trimmed = trim_trailing_newline(line);
        if trimmed.is_empty() {
            out.extend_from_slice(line);
            continue;
        }
        match serde_json::from_slice::<Value>(trimmed) {
            Ok(value) => match timestamp_from_value(value.get("timestamp")) {
                Some(parsed) if parsed > boundary => {}
                _ => out.extend_from_slice(line),
            },
            Err(_) => out.extend_from_slice(line),
        }
    }
    out
}

fn truncate_opencode_messages_after(
    transcript_data: &[u8],
    boundary: DateTime<Utc>,
) -> Result<Vec<u8>> {
    let mut value: Value = match serde_json::from_slice(transcript_data) {
        Ok(v) => v,
        Err(_) => return Ok(transcript_data.to_vec()),
    };
    let Some(messages) = value.get_mut("messages").and_then(Value::as_array_mut) else {
        return Ok(transcript_data.to_vec());
    };
    messages.retain(|message| match opencode_message_timestamp(message) {
        Some(parsed) => parsed <= boundary,
        None => true,
    });
    serde_json::to_vec(&value).context("re-serialise truncated opencode transcript")
}

fn opencode_message_timestamp(message: &Value) -> Option<DateTime<Utc>> {
    let time = message.get("info")?.get("time")?;
    timestamp_from_value(time.get("created"))
        .or_else(|| timestamp_from_value(time.get("completed")))
}

fn timestamp_from_value(value: Option<&Value>) -> Option<DateTime<Utc>> {
    match value? {
        Value::String(raw) => parse_timestamp_str(raw),
        Value::Number(raw) => raw
            .as_i64()
            .or_else(|| raw.as_u64().and_then(|value| i64::try_from(value).ok()))
            .and_then(datetime_from_epoch),
        _ => None,
    }
}

fn parse_timestamp_str(raw: &str) -> Option<DateTime<Utc>> {
    raw.parse::<DateTime<Utc>>()
        .ok()
        .or_else(|| raw.parse::<i64>().ok().and_then(datetime_from_epoch))
}

fn datetime_from_epoch(raw: i64) -> Option<DateTime<Utc>> {
    if raw.unsigned_abs() >= 100_000_000_000 {
        DateTime::<Utc>::from_timestamp_millis(raw)
    } else {
        DateTime::<Utc>::from_timestamp(raw, 0)
    }
}

fn trim_trailing_newline(line: &[u8]) -> &[u8] {
    let mut end = line.len();
    while end > 0 && (line[end - 1] == b'\n' || line[end - 1] == b'\r') {
        end -= 1;
    }
    &line[..end]
}

#[cfg(test)]
mod tests {
    use super::{
        super::stable_promoted::{
            COPILOT_STABLE_PROMOTED_SPEC, CURSOR_STABLE_PROMOTED_SPEC,
            OPENCODE_STABLE_PROMOTED_SPEC,
        },
        *,
    };

    #[test]
    fn jsonl_timestamp_truncator_drops_post_boundary_lines() {
        let agent = StablePromotedAgent(&CURSOR_STABLE_PROMOTED_SPEC);
        let input = b"{\"timestamp\":\"2026-05-05T10:00:00Z\",\"text\":\"keep\"}\n\
                      {\"timestamp\":\"2026-05-05T11:00:00Z\",\"text\":\"drop\"}\n\
                      {\"text\":\"keep-missing-timestamp\"}\n\
                      not json\n";

        let output = agent
            .truncate_transcript(input, "2026-05-05T10:30:00Z")
            .unwrap();
        let text = String::from_utf8(output).unwrap();

        assert!(text.contains("keep"));
        assert!(text.contains("keep-missing-timestamp"));
        assert!(text.contains("not json"));
        assert!(!text.contains("drop"));
    }

    #[test]
    fn jsonl_timestamp_truncator_accepts_numeric_epoch_millis() {
        let agent = StablePromotedAgent(&COPILOT_STABLE_PROMOTED_SPEC);
        let input = b"{\"timestamp\":1777975200000,\"text\":\"keep\"}\n\
                      {\"timestamp\":1777978800000,\"text\":\"drop\"}\n";

        let output = agent
            .truncate_transcript(input, "2026-05-05T10:30:00Z")
            .unwrap();
        let text = String::from_utf8(output).unwrap();

        assert!(text.contains("keep"));
        assert!(!text.contains("drop"));
    }

    #[test]
    fn opencode_truncator_drops_messages_created_after_boundary() {
        let agent = StablePromotedAgent(&OPENCODE_STABLE_PROMOTED_SPEC);
        let input = br#"{
            "info": {"id": "ses_1"},
            "messages": [
                {"info": {"role": "user", "time": {"created": 1777975200000}}, "parts": [{"text": "keep"}]},
                {"info": {"role": "assistant", "time": {"created": 1777978800000}}, "parts": [{"text": "drop"}]},
                {"info": {"role": "system"}, "parts": [{"text": "keep-missing-time"}]}
            ]
        }"#;

        let output = agent
            .truncate_transcript(input, "2026-05-05T10:30:00Z")
            .unwrap();
        let value: Value = serde_json::from_slice(&output).unwrap();
        let messages = value["messages"].as_array().unwrap();

        assert_eq!(messages.len(), 2);
        assert!(output.windows("keep".len()).any(|window| window == b"keep"));
        assert!(!output.windows("drop".len()).any(|window| window == b"drop"));
    }
}
