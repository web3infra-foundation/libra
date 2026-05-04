#![allow(dead_code)]

use std::time::Duration;

use anyhow::{Context, Result};
use serde_json::Value;

use super::CodeSession;

pub struct Scenario<'a> {
    name: String,
    session: &'a mut CodeSession,
}

impl<'a> Scenario<'a> {
    pub fn new(name: impl Into<String>, session: &'a mut CodeSession) -> Self {
        Self {
            name: name.into(),
            session,
        }
    }

    pub fn step<'s>(&'s mut self, name: impl Into<String>) -> ScenarioStep<'s, 'a> {
        ScenarioStep {
            scenario: self,
            name: name.into(),
        }
    }
}

pub struct ScenarioStep<'s, 'a> {
    scenario: &'s mut Scenario<'a>,
    name: String,
}

impl<'s, 'a> ScenarioStep<'s, 'a> {
    pub fn attach_automation(mut self, client_id: &str) -> Result<Self> {
        self.with_context(|session| session.attach_automation(client_id))?;
        Ok(self)
    }

    pub fn submit(mut self, text: &str) -> Result<Self> {
        self.with_context(|session| session.submit_message(text).map(|_| ()))?;
        Ok(self)
    }

    pub fn reclaim_via_tui_command(mut self) -> Result<Self> {
        self.with_context(|session| session.write_tui_line("/control reclaim"))?;
        Ok(self)
    }

    pub fn expect_controller_kind(mut self, expected: &str) -> Result<Self> {
        self.with_context(|session| {
            session
                .wait_for_snapshot(Duration::from_secs(10), |snapshot| {
                    controller_kind(snapshot) == Some(expected)
                })
                .map(|_| ())
        })?;
        Ok(self)
    }

    pub fn expect_status_eq(mut self, expected: &str) -> Result<Self> {
        self.with_context(|session| {
            session
                .wait_for_snapshot(Duration::from_secs(10), |snapshot| {
                    status(snapshot) == Some(expected)
                })
                .map(|_| ())
        })?;
        Ok(self)
    }

    pub fn expect_transcript_contains(mut self, needle: &str) -> Result<Self> {
        self.with_context(|session| {
            session
                .wait_for_snapshot(Duration::from_secs(20), |snapshot| {
                    transcript_contains(snapshot, needle)
                })
                .map(|_| ())
        })?;
        Ok(self)
    }

    fn with_context<F, T>(&mut self, f: F) -> Result<T>
    where
        F: FnOnce(&mut CodeSession) -> Result<T>,
    {
        let result = f(self.scenario.session);
        result.with_context(|| {
            format!(
                "scenario '{}' step '{}' failed\n{}",
                self.scenario.name,
                self.name,
                self.scenario.session.debug_context()
            )
        })
    }
}

fn status(snapshot: &Value) -> Option<&str> {
    snapshot.get("status").and_then(Value::as_str)
}

fn controller_kind(snapshot: &Value) -> Option<&str> {
    snapshot
        .get("controller")
        .and_then(|controller| controller.get("kind"))
        .and_then(Value::as_str)
}

fn transcript_contains(snapshot: &Value, needle: &str) -> bool {
    snapshot
        .get("transcript")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.get("content").and_then(Value::as_str))
        .any(|content| content.contains(needle))
}
