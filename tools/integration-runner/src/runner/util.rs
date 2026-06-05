use std::{
    collections::BTreeSet,
    env,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, bail};

pub(super) fn parse_waves(waves: Option<String>) -> Result<BTreeSet<u8>> {
    let input = waves.unwrap_or_else(|| "0".to_string());
    input
        .split(',')
        .filter(|item| !item.trim().is_empty())
        .map(|item| {
            item.trim()
                .parse::<u8>()
                .with_context(|| format!("invalid wave {item}"))
        })
        .collect()
}

pub(super) fn parse_only(only: Option<String>) -> BTreeSet<String> {
    only.unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

pub(super) fn ensure_binary(repo_root: &Path, binary: &Path) -> Result<()> {
    if binary.exists() {
        return Ok(());
    }
    let status = Command::new("cargo")
        .args(["build", "--bin", "libra"])
        .current_dir(repo_root)
        .env("LIBRA_SKIP_WEB_BUILD", "1")
        .status()
        .context("spawn cargo build --bin libra")?;
    if !status.success() {
        bail!("cargo build --bin libra failed with status {status}");
    }
    if !binary.exists() {
        bail!(
            "expected binary does not exist after build: {}",
            binary.display()
        );
    }
    Ok(())
}

pub(super) fn safe_path() -> String {
    let mut paths = vec![
        "/usr/bin".to_string(),
        "/bin".to_string(),
        "/usr/sbin".to_string(),
        "/sbin".to_string(),
    ];
    for tool in ["git", "ssh"] {
        if let Some(parent) = find_on_host_path(tool)
            .and_then(|path| path.parent().and_then(Path::to_str).map(ToOwned::to_owned))
            && !paths.iter().any(|path| path == &parent)
        {
            paths.push(parent);
        }
    }
    paths.join(":")
}

pub(super) fn find_on_host_path(tool: &str) -> Option<PathBuf> {
    env::var_os("PATH").and_then(|path| {
        env::split_paths(&path)
            .map(|dir| dir.join(tool))
            .find(|candidate| candidate.is_file())
    })
}
