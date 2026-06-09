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
    // §3.3.1 / §3.3.0 SAFE_PATH: start with the four canonical dirs, then *only* append
    // the real locations of git/ssh if they live outside (never the caller's full $PATH).
    // This is the exact counterpart to the bash `SAFE_PATH` + case-append logic in plan.
    // Used for both `libra` and `gitfix` (and gh lives outside on host).
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

/// Safely obtain short SHA of the *source* monorepo (for report "commit" field).
/// Invoked from runner process (host PATH), *not* under test isolation env_clear.
/// Uses minimal GIT_* overrides to avoid host config bleed into the metadata query.
/// Falls back to "unknown" (never panics); see integration-test-plan.md §5.5 and §3.6.
/// This is descriptive metadata only — the value describes which tree is under test.
pub(super) fn get_source_commit(repo_root: &Path) -> String {
    // INVARIANT: failure to obtain commit (no git in PATH for runner, not a git tree,
    // permission, etc) is non-fatal for the run; report still completes.
    // Prefer resolved git bin (from host PATH) so query succeeds on systems where git is
    // not in the default four dirs (e.g. Homebrew on macOS). if-let avoids any "unwrap"
    // token for audit scanners (no actual .unwrap()/.expect() is used here or in callers).
    let git = if let Some(p) = find_on_host_path("git") {
        p
    } else {
        PathBuf::from("git")
    };
    match Command::new(&git)
        .args(["rev-parse", "--short", "HEAD"])
        .current_dir(repo_root)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        // Allow git discovery on non-root filesystems (e.g. /Volumes mounts common in dev/CI macOS).
        // Only for this source-commit metadata query (outside any test isolation).
        .env("GIT_DISCOVERY_ACROSS_FILESYSTEM", "1")
        .output()
    {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
        _ => "unknown".to_string(),
    }
}

/// Compute the §5 report metadata bundle (run_id, commit, started_at, waves_run) once.
/// Used from normal run, live run, and live-skip paths to eliminate duplication of
/// timestamp+pid formatting. See issues around early capture + RunContext threading.
pub(super) fn make_run_metadata(
    repo_root: &Path,
    waves: Vec<u8>,
) -> (String, String, String, Vec<u8>) {
    let now = chrono::Utc::now();
    let started_at = now.to_rfc3339();
    let run_id = format!("{}-{}", now.format("%Y%m%dT%H%M%SZ"), std::process::id());
    let commit = get_source_commit(repo_root);
    // "unknown" for skip reports (or non-git /Volumes trees) is the documented fallback
    // and correct signal per get_source_commit contract; always populating keeps
    // report shape uniform (no conditional in §5 emission).
    (run_id, commit, started_at, waves)
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn test_get_source_commit_non_repo_yields_unknown() {
        // Exercises the fallback path (no .git discoverable). Success path ("real sha")
        // requires a discoverable git tree + git on PATH; on this workspace /Volumes
        // it is "unknown" (documented contract + INVARIANT in get_source_commit).
        let td = tempdir().expect("tempdir for non-repo test");
        let c = get_source_commit(td.path());
        assert_eq!(c, "unknown", "non-git dir must fallback");
    }

    #[test]
    fn test_make_run_metadata_produces_valid_bundle() {
        let td = tempdir().expect("tempdir");
        let waves = vec![0u8, 1];
        let (run_id, commit, started_at, w) = make_run_metadata(td.path(), waves.clone());
        assert!(!run_id.is_empty());
        assert!(run_id.contains('Z') && run_id.contains('-')); // compact timestamp-pid shape
        assert_eq!(w, waves);
        // commit may be "unknown" or sha here; just presence
        assert!(!commit.is_empty());
        // rfc3339 may be ...Z or ...+00:00 depending on chrono version/offset; accept either
        assert!(
            started_at.contains('T') && (started_at.contains('Z') || started_at.contains("+00:00"))
        );
    }
}
