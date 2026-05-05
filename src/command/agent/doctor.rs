//! `libra agent doctor` — read-only diagnostics. Surfaces hook installation
//! state, stuck active sessions, and orphan checkpoints so operators can
//! see where Libra and the captured agents have drifted out of sync.
//!
//! V1 returns a structured report. JSON output is suitable for scripted
//! monitoring; the human path prints a category summary plus a per-issue
//! line for anything non-zero.

use sea_orm::{ConnectionTrait, Statement};
use serde::Serialize;

use super::DoctorArgs;
use crate::{
    internal::{
        ai::{
            hooks::providers::{claude_provider, gemini_provider},
            observed_agents::{AgentStability, PREVIEW_SPECS},
        },
        db::get_db_conn_instance,
    },
    utils::{
        error::{CliError, CliResult},
        output::{OutputConfig, emit_json_data},
    },
};

#[derive(Debug, Serialize)]
struct ProviderHookStatus {
    name: &'static str,
    /// Adapter stability tier — `Stable` adapters carry a real
    /// `HookProvider` and report installation status; `Preview` ones
    /// (Phase 3.1) surface as "not yet installable".
    tier: AgentStability,
    installed: Option<bool>,
    error: Option<String>,
}

#[derive(Debug, Serialize)]
struct DoctorReport {
    schema_present: bool,
    active_sessions: i64,
    stopped_sessions: i64,
    orphan_checkpoints: i64,
    provider_hooks: Vec<ProviderHookStatus>,
}

pub async fn execute_safe(_args: DoctorArgs, output: &OutputConfig) -> CliResult<()> {
    let conn = get_db_conn_instance().await;
    let schema_present = table_exists(&conn, "agent_session").await?
        && table_exists(&conn, "agent_checkpoint").await?;

    let (active_sessions, stopped_sessions, orphan_checkpoints) = if schema_present {
        let active = scalar_count(
            &conn,
            "SELECT COUNT(*) AS n FROM agent_session WHERE state = 'active'",
        )
        .await?;
        let stopped = scalar_count(
            &conn,
            "SELECT COUNT(*) AS n FROM agent_session WHERE state = 'stopped'",
        )
        .await?;
        // Orphan = checkpoint rows whose session_id no longer joins (would
        // imply CASCADE failed or the row was hand-written). Should be 0
        // under normal operation; surfacing >0 is a real diagnostic.
        let orphans = scalar_count(
            &conn,
            "SELECT COUNT(*) AS n FROM agent_checkpoint cp \
             LEFT JOIN agent_session s ON s.session_id = cp.session_id \
             WHERE s.session_id IS NULL",
        )
        .await?;
        (active, stopped, orphans)
    } else {
        (0, 0, 0)
    };

    // Hook installation status for the two stable providers, followed by
    // the five preview adapters from `observed_agents::preview`. Preview
    // entries surface as `tier: AgentStability::Preview` with
    // `installed: None` so operators can see the full v1 matrix from one
    // command.
    let mut provider_hooks = vec![
        check_provider(
            "claude-code",
            AgentStability::Stable,
            Some(claude_provider()),
        ),
        check_provider("gemini", AgentStability::Stable, Some(gemini_provider())),
    ];
    for spec in PREVIEW_SPECS {
        provider_hooks.push(check_provider(
            spec.provider_name,
            AgentStability::Preview,
            None,
        ));
    }

    emit_report(
        &DoctorReport {
            schema_present,
            active_sessions,
            stopped_sessions,
            orphan_checkpoints,
            provider_hooks,
        },
        output,
    )
}

fn check_provider(
    name: &'static str,
    tier: AgentStability,
    provider: Option<&dyn crate::internal::ai::hooks::provider::HookProvider>,
) -> ProviderHookStatus {
    let Some(provider) = provider else {
        // Preview adapters don't carry a HookProvider yet. Surface them
        // explicitly as preview/unknown so the report is still complete.
        return ProviderHookStatus {
            name,
            tier,
            installed: None,
            error: None,
        };
    };
    match provider.hooks_are_installed() {
        Ok(installed) => ProviderHookStatus {
            name,
            tier,
            installed: Some(installed),
            error: None,
        },
        Err(err) => ProviderHookStatus {
            name,
            tier,
            installed: None,
            error: Some(err.to_string()),
        },
    }
}

fn emit_report(report: &DoctorReport, output: &OutputConfig) -> CliResult<()> {
    if output.is_json() {
        return emit_json_data("agent_doctor", report, output);
    }
    if output.quiet {
        return Ok(());
    }
    println!(
        "Schema present       : {}",
        if report.schema_present { "yes" } else { "no" }
    );
    println!("Active sessions      : {}", report.active_sessions);
    println!("Stopped sessions     : {}", report.stopped_sessions);
    println!("Orphan checkpoints   : {}", report.orphan_checkpoints);

    println!("Provider hooks:");
    for ph in &report.provider_hooks {
        let tier_tag = match ph.tier {
            AgentStability::Preview => " [preview]",
            AgentStability::Stable => "",
        };
        match (ph.installed, &ph.error) {
            (Some(true), _) => println!("  {}{tier_tag}: installed", ph.name),
            (Some(false), _) => println!("  {}{tier_tag}: NOT installed", ph.name),
            (None, Some(err)) => println!("  {}{tier_tag}: error — {err}", ph.name),
            (None, None) => println!("  {}{tier_tag}: not yet installable", ph.name),
        }
    }

    if report.orphan_checkpoints > 0 {
        println!(
            "Hint: orphan checkpoints indicate broken FK cascade — \
             consider `libra agent clean --all`."
        );
    }
    if !report.schema_present {
        println!("Hint: run `libra init` to apply pending migrations.");
    }
    Ok(())
}

async fn table_exists(conn: &(impl ConnectionTrait + ?Sized), name: &str) -> CliResult<bool> {
    let backend = conn.get_database_backend();
    conn.query_one(Statement::from_sql_and_values(
        backend,
        "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ? LIMIT 1",
        [name.into()],
    ))
    .await
    .map(|row| row.is_some())
    .map_err(|e| CliError::fatal(format!("failed to query sqlite_master: {e}")))
}

async fn scalar_count(conn: &(impl ConnectionTrait + ?Sized), sql: &str) -> CliResult<i64> {
    let backend = conn.get_database_backend();
    let row = conn
        .query_one(Statement::from_sql_and_values(backend, sql, []))
        .await
        .map_err(|e| CliError::fatal(format!("doctor query failed: {e}")))?
        .ok_or_else(|| CliError::fatal("doctor count returned no rows".to_string()))?;
    row.try_get_by::<i64, _>("n")
        .map_err(|e| CliError::fatal(format!("failed to decode doctor count: {e}")))
}
