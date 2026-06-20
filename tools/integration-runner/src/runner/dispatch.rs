use std::time::Instant;

use anyhow::Result;

use super::types::{RunContext, ScenarioCtx, ScenarioResult};
use crate::registry::ScenarioFn;

pub(super) fn run_wave0(ctx: &RunContext) -> ScenarioResult {
    let start = Instant::now();
    let run_dir = ctx.run_root.join("repos/wave0.build-and-help");
    let mut sctx = ScenarioCtx::new(ctx, "wave0.build-and-help", 0, run_dir);
    let result = (|| -> Result<()> {
        sctx.command(&["--version"], sctx.run_dir.clone(), true)?;
        sctx.command(&["--help"], sctx.run_dir.clone(), true)?;
        Ok(())
    })();
    sctx.finish(start, result)
}

pub(super) fn run_scenario(
    ctx: &RunContext,
    id: &str,
    wave: u8,
    scenario: ScenarioFn,
) -> ScenarioResult {
    let start = Instant::now();
    let run_dir = ctx.run_root.join("repos").join(id);
    let mut sctx = ScenarioCtx::new(ctx, id, wave, run_dir);
    let result = scenario(&mut sctx);
    sctx.finish(start, result)
}

pub(super) fn skip_result(id: &str, wave: u8, ctx: &RunContext, reason: &str) -> ScenarioResult {
    ScenarioResult {
        id: id.to_string(),
        wave,
        status: "skipped".to_string(),
        duration_ms: 0,
        run_dir: ctx.run_root.join("repos").join(id).display().to_string(),
        commands: Vec::new(),
        error: Some(reason.to_string()),
        cleanup: None,
    }
}
