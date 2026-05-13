//! `libra sandbox` diagnostics command surface.

use std::{
    env,
    io::Write,
    path::{Path, PathBuf},
};

use clap::{Parser, Subcommand};
use serde::Serialize;

use crate::{
    info_println,
    internal::ai::sandbox::{SandboxEnforcement, SandboxPolicy},
    utils::{
        error::{CliError, CliResult},
        output::{OutputConfig, emit_json_data},
    },
};

const SANDBOX_ENFORCEMENT_ENV: &str = "LIBRA_SANDBOX_ENFORCEMENT";
const LINUX_SANDBOX_EXE_ENV: &str = "LIBRA_LINUX_SANDBOX_EXE";
const LINUX_SANDBOX_BWRAP_ENV: &str = "LIBRA_USE_LINUX_SANDBOX_BWRAP";
const MACOS_SEATBELT_EXECUTABLE: &str = "/usr/bin/sandbox-exec";

#[derive(Parser, Debug)]
pub struct SandboxArgs {
    #[command(subcommand)]
    pub command: SandboxSubcommand,
}

#[derive(Subcommand, Debug)]
pub enum SandboxSubcommand {
    /// Show effective sandbox diagnostics for AI tool execution.
    Status,
}

#[derive(Debug, Serialize)]
struct SandboxStatusOutput {
    platform: &'static str,
    sandbox_type: &'static str,
    enforcement: &'static str,
    effective_enforcement: &'static str,
    writable_roots: Vec<String>,
    network: SandboxNetworkStatus,
    proxy_backend: &'static str,
    bwrap_available: bool,
    bwrap_requested: bool,
    seatbelt_available: bool,
    helper_path: SandboxHelperPathStatus,
    warnings: Vec<String>,
}

#[derive(Debug, Serialize)]
struct SandboxNetworkStatus {
    mode: &'static str,
    allowlist: Vec<String>,
}

#[derive(Debug, Serialize)]
struct SandboxHelperPathStatus {
    path: Option<String>,
    exists: bool,
}

pub async fn execute_safe(args: SandboxArgs, output: &OutputConfig) -> CliResult<()> {
    match args.command {
        SandboxSubcommand::Status => status(output).await,
    }
}

async fn status(output: &OutputConfig) -> CliResult<()> {
    let report = build_status_report()?;
    if output.is_json() {
        return emit_json_data("sandbox.status", &report, output);
    }
    render_status_human(&report, output)
}

fn build_status_report() -> CliResult<SandboxStatusOutput> {
    let cwd = env::current_dir().map_err(|error| {
        CliError::io(format!(
            "failed to inspect sandbox status from current directory: {error}"
        ))
    })?;
    let policy = SandboxPolicy::default();
    let writable_roots = policy
        .get_writable_roots_with_cwd(&cwd)
        .into_iter()
        .map(|root| root.root.display().to_string())
        .collect();
    let helper_path = env_path(LINUX_SANDBOX_EXE_ENV);
    let helper_exists = helper_path.as_deref().is_some_and(executable_file_exists);
    let bwrap_available = path_executable_available("bwrap");
    let bwrap_requested = env_flag_enabled(LINUX_SANDBOX_BWRAP_ENV);
    let seatbelt_available = executable_file_exists(Path::new(MACOS_SEATBELT_EXECUTABLE));
    let mut warnings = Vec::new();
    let enforcement = env_sandbox_enforcement(&mut warnings);
    let sandbox_type = match env::consts::OS {
        "macos" if seatbelt_available => "macos-seatbelt",
        "macos" => {
            warnings.push(
                "macOS sandbox-exec is not available; AI shell commands cannot use Seatbelt"
                    .to_string(),
            );
            "none"
        }
        "linux" if helper_exists => "linux-seccomp",
        "linux" => {
            if let Some(path) = &helper_path {
                if enforcement.requires_effective_sandbox() {
                    warnings.push(format!(
                        "linux sandbox helper '{}' is not executable; AI shell commands that require Libra's internal sandbox will fail",
                        path.display()
                    ));
                } else {
                    warnings.push(format!(
                        "linux sandbox helper '{}' is not executable; AI shell commands cannot enter the configured helper sandbox",
                        path.display()
                    ));
                }
            } else {
                if enforcement.requires_effective_sandbox() {
                    warnings.push(
                        "linux sandbox helper is not configured; AI shell commands that require Libra's internal sandbox will fail"
                            .to_string(),
                    );
                } else {
                    warnings.push(
                        "linux sandbox helper is not configured; AI shell commands currently fall back to no OS sandbox"
                            .to_string(),
                    );
                }
            }
            "none"
        }
        "windows" => {
            warnings.push(
                "Windows restricted-token sandbox is not implemented in the current runtime"
                    .to_string(),
            );
            "windows-restricted-token-unimplemented"
        }
        _ => {
            warnings.push(format!(
                "OS sandbox execution is not supported on platform '{}'",
                env::consts::OS
            ));
            "none"
        }
    };
    if bwrap_requested && !bwrap_available {
        warnings.push(
            "LIBRA_USE_LINUX_SANDBOX_BWRAP is enabled but bwrap is not available on PATH"
                .to_string(),
        );
    }

    Ok(SandboxStatusOutput {
        platform: env::consts::OS,
        sandbox_type,
        enforcement: enforcement.as_str(),
        effective_enforcement: enforcement.as_str(),
        writable_roots,
        network: SandboxNetworkStatus {
            mode: if policy.has_full_network_access() {
                "full"
            } else {
                "denied"
            },
            allowlist: Vec::new(),
        },
        proxy_backend: "none",
        bwrap_available,
        bwrap_requested,
        seatbelt_available,
        helper_path: SandboxHelperPathStatus {
            path: helper_path.map(|path| path.display().to_string()),
            exists: helper_exists,
        },
        warnings,
    })
}

fn render_status_human(report: &SandboxStatusOutput, output: &OutputConfig) -> CliResult<()> {
    info_println!(output, "Sandbox status");
    info_println!(output, "  platform: {}", report.platform);
    info_println!(output, "  sandbox_type: {}", report.sandbox_type);
    info_println!(output, "  enforcement: {}", report.enforcement);
    info_println!(
        output,
        "  effective_enforcement: {}",
        report.effective_enforcement
    );
    info_println!(output, "  network: {}", report.network.mode);
    info_println!(output, "  proxy_backend: {}", report.proxy_backend);
    info_println!(output, "  bwrap_available: {}", report.bwrap_available);
    info_println!(output, "  bwrap_requested: {}", report.bwrap_requested);
    info_println!(
        output,
        "  seatbelt_available: {}",
        report.seatbelt_available
    );
    let helper_path = report
        .helper_path
        .path
        .as_deref()
        .unwrap_or("(not configured)");
    info_println!(output, "  helper_path: {helper_path}");
    info_println!(
        output,
        "  helper_path_exists: {}",
        report.helper_path.exists
    );
    info_println!(output, "  writable_roots:");
    for root in &report.writable_roots {
        info_println!(output, "    - {root}");
    }
    if !report.warnings.is_empty() {
        info_println!(output, "  warnings:");
        for warning in &report.warnings {
            info_println!(output, "    - {warning}");
        }
    }
    std::io::stdout()
        .flush()
        .map_err(|error| CliError::io(format!("failed to write sandbox status: {error}")))?;
    Ok(())
}

fn env_path(name: &str) -> Option<PathBuf> {
    env::var_os(name).and_then(|value| {
        if value.to_string_lossy().is_empty() {
            None
        } else {
            Some(PathBuf::from(value))
        }
    })
}

fn env_flag_enabled(name: &str) -> bool {
    env::var_os(name).is_some_and(|value| {
        let value = value.to_string_lossy().to_ascii_lowercase();
        matches!(value.as_str(), "1" | "true" | "yes" | "on")
    })
}

fn env_sandbox_enforcement(warnings: &mut Vec<String>) -> SandboxEnforcement {
    let Ok(value) = env::var(SANDBOX_ENFORCEMENT_ENV) else {
        return SandboxEnforcement::default();
    };

    match value.parse::<SandboxEnforcement>() {
        Ok(enforcement) => enforcement,
        Err(error) => {
            warnings.push(error.to_string());
            SandboxEnforcement::default()
        }
    }
}

fn path_executable_available(binary: &str) -> bool {
    env::var_os("PATH").is_some_and(|path| {
        env::split_paths(&path).any(|entry| executable_file_exists(&entry.join(binary)))
    })
}

fn executable_file_exists(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        path.metadata()
            .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        true
    }
}
