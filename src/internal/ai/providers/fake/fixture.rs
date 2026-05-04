//! Fixture schema for the test-only fake provider.

use std::{fs, path::Path, time::Duration};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum FakeFixtureError {
    #[error("failed to read fake provider fixture '{path}': {source}")]
    Read {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse fake provider fixture '{path}': {source}")]
    Parse {
        path: String,
        #[source]
        source: serde_json::Error,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FakeFixture {
    #[serde(default)]
    pub responses: Vec<FakeResponseRule>,
    #[serde(default)]
    pub fallback: Option<FakeResponseAction>,
}

impl FakeFixture {
    pub fn from_path(path: &Path) -> Result<Self, FakeFixtureError> {
        let text = fs::read_to_string(path).map_err(|source| FakeFixtureError::Read {
            path: path.display().to_string(),
            source,
        })?;
        serde_json::from_str(&text).map_err(|source| FakeFixtureError::Parse {
            path: path.display().to_string(),
            source,
        })
    }

    pub fn select<'a>(
        &'a self,
        latest_user_text: &str,
    ) -> Option<(Option<usize>, &'a FakeResponseAction)> {
        self.responses
            .iter()
            .enumerate()
            .find(|(_, rule)| rule.matcher.matches(latest_user_text))
            .map(|(index, rule)| (Some(index), &rule.action))
            .or_else(|| self.fallback.as_ref().map(|action| (None, action)))
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FakeResponseRule {
    #[serde(default, rename = "match")]
    pub matcher: FakeMatcher,
    #[serde(flatten)]
    pub action: FakeResponseAction,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FakeMatcher {
    #[serde(default)]
    pub contains: Option<String>,
    #[serde(default)]
    pub equals: Option<String>,
}

impl FakeMatcher {
    fn matches(&self, latest_user_text: &str) -> bool {
        let contains = self
            .contains
            .as_ref()
            .is_none_or(|needle| latest_user_text.contains(needle));
        let equals = self
            .equals
            .as_ref()
            .is_none_or(|expected| latest_user_text == expected);
        contains && equals
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FakeResponseAction {
    Text {
        text: String,
        #[serde(default, rename = "delayMs", alias = "delay_ms")]
        delay_ms: u64,
        #[serde(default)]
        stream: Vec<FakeStreamDelta>,
    },
    ToolCall {
        id: String,
        name: String,
        #[serde(default)]
        arguments: Value,
        #[serde(default, rename = "delayMs", alias = "delay_ms")]
        delay_ms: u64,
        #[serde(default)]
        stream: Vec<FakeStreamDelta>,
    },
    Error {
        message: String,
        #[serde(default, rename = "delayMs", alias = "delay_ms")]
        delay_ms: u64,
    },
}

impl Default for FakeResponseAction {
    fn default() -> Self {
        Self::Error {
            message: "no fake provider response matched".to_string(),
            delay_ms: 0,
        }
    }
}

impl FakeResponseAction {
    pub fn delay(&self) -> Duration {
        let millis = match self {
            Self::Text { delay_ms, .. }
            | Self::ToolCall { delay_ms, .. }
            | Self::Error { delay_ms, .. } => *delay_ms,
        };
        Duration::from_millis(millis)
    }

    pub fn stream(&self) -> &[FakeStreamDelta] {
        match self {
            Self::Text { stream, .. } | Self::ToolCall { stream, .. } => stream,
            Self::Error { .. } => &[],
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FakeStreamDelta {
    Text { delta: String },
    Thinking { delta: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_selects_first_matching_response() {
        let fixture = FakeFixture {
            responses: vec![FakeResponseRule {
                matcher: FakeMatcher {
                    contains: Some("hello".to_string()),
                    equals: None,
                },
                action: FakeResponseAction::Text {
                    text: "hi".to_string(),
                    delay_ms: 0,
                    stream: vec![],
                },
            }],
            fallback: None,
        };

        let (index, action) = fixture.select("say hello").expect("match should exist");
        assert_eq!(index, Some(0));
        assert!(matches!(action, FakeResponseAction::Text { text, .. } if text == "hi"));
    }

    #[test]
    fn fixture_accepts_camel_case_delay_ms() {
        let fixture: FakeFixture = serde_json::from_value(serde_json::json!({
            "responses": [
                {
                    "match": { "contains": "slow" },
                    "type": "text",
                    "delayMs": 10000,
                    "text": "delayed"
                }
            ]
        }))
        .expect("fixture should parse");

        let (_, action) = fixture.select("slow request").expect("match should exist");

        assert_eq!(action.delay(), Duration::from_secs(10));
    }
}
