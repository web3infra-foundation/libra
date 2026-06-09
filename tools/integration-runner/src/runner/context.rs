use std::{path::PathBuf, time::Instant};

use anyhow::Result;

use super::types::{RunContext, ScenarioCtx, ScenarioResult};

impl<'a> ScenarioCtx<'a> {
    pub(super) fn new(run: &'a RunContext, id: &str, wave: u8, run_dir: PathBuf) -> Self {
        Self {
            run,
            id: id.to_string(),
            wave,
            run_dir,
            commands: Vec::new(),
            seq: 0,
            cleanup_status: None,
        }
    }
    pub(crate) fn set_cleanup(&mut self, status: &str) {
        self.cleanup_status = Some(status.to_string());
    }
    pub(super) fn finish(self, start: Instant, result: Result<()>) -> ScenarioResult {
        let (status, error) = match result {
            Ok(()) => ("passed".to_string(), None),
            Err(err) => ("failed".to_string(), Some(format!("{err:#}"))),
        };
        ScenarioResult {
            id: self.id,
            wave: self.wave,
            status,
            duration_ms: start.elapsed().as_millis(),
            run_dir: self.run_dir.display().to_string(),
            commands: self.commands,
            error,
            cleanup: self.cleanup_status,
        }
    }
    pub(crate) fn repo(&self, name: &str) -> PathBuf {
        self.run_dir.join(name)
    }
}
