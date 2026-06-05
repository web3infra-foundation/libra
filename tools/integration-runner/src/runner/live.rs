use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, bail};

use super::{
    dispatch::{run_scenario, skip_result},
    types::RunContext,
    util::{ensure_binary, find_on_host_path, parse_only, safe_path},
};
use crate::{
    manifest::load_manifest,
    registry::scenario_registry,
    support::{redact, tail, write_report},
};

pub(crate) fn run_live(
    repo_root: &Path,
    only: Option<String>,
    binary: Option<PathBuf>,
    keep: bool,
) -> Result<()> {
    // Preflight gh presence + auth (must succeed to even consider creating temp repos).
    let gh_bin = find_on_host_path("gh");
    if gh_bin.is_none() {
        return write_live_skip_report(
            repo_root,
            only,
            binary,
            "gh CLI not found on PATH (install gh and ensure `gh auth login`)",
            "all live scenarios skipped (no gh)",
        );
    }

    // gh present: run auth status (do not leak token; status is safe to print)
    let auth_status = Command::new("gh")
        .args(["auth", "status", "--active", "--hostname", "github.com"])
        .output()
        .context("run gh auth status")?;
    if !auth_status.status.success() {
        let reason = format!(
            "gh auth status failed (stderr: {}) — run `gh auth login --hostname github.com`",
            tail(&String::from_utf8_lossy(&auth_status.stderr), 200)
        );
        return write_live_skip_report(
            repo_root,
            only,
            binary,
            &reason,
            "all live scenarios skipped (gh not authenticated)",
        );
    }

    // Additional preflight for delete_repo scope (required to honor "if no delete perm, do not start").
    // Use -t but NEVER persist raw to logs; inspect in-memory only (redact before any decision log).
    let auth_with_token = Command::new("gh")
        .args(["auth", "status", "-t", "--hostname", "github.com"])
        .output()
        .context("run gh auth status -t for scope probe")?;
    let raw_token_status = String::from_utf8_lossy(&auth_with_token.stdout).to_string()
        + &String::from_utf8_lossy(&auth_with_token.stderr);
    // Redact in memory for inspection; do not write raw_token_status anywhere.
    let redacted_status = redact(&raw_token_status);
    if !redacted_status.contains("delete_repo") {
        let reason = "gh token missing 'delete_repo' scope (create succeeded but delete would 403); run `gh auth refresh -h github.com -s delete_repo` then retry";
        return write_live_skip_report(
            repo_root,
            only,
            binary,
            reason,
            "all live scenarios skipped (missing delete_repo scope)",
        );
    }

    // Auth ok — proceed to full isolated run (same layout as normal run)
    let manifest = load_manifest(repo_root)?;
    let binary = binary.unwrap_or_else(|| repo_root.join("target/debug/libra"));
    ensure_binary(repo_root, &binary)?;

    let run_root = tempfile::Builder::new()
        .prefix("libra-integ-live-")
        .tempdir()
        .context("create live run root")?
        .keep();
    for dir in [
        "home",
        "xdg-config",
        "xdg-cache",
        "repos",
        "fixtures",
        "logs",
        "artifacts",
        "tmp",
    ] {
        fs::create_dir_all(run_root.join(dir)).with_context(|| format!("create {dir}"))?;
    }
    let safe_path = safe_path();
    let results_path = run_root.join("results.ndjson");
    let ctx = RunContext {
        run_root: run_root.clone(),
        binary,
        safe_path,
        results_path,
    };

    let by_id: BTreeMap<_, _> = manifest
        .scenarios
        .iter()
        .map(|scenario| (scenario.id.as_str(), scenario))
        .collect();

    let selected_ids: Vec<String> = if let Some(o) = only {
        o.split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .collect()
    } else {
        manifest
            .scenarios
            .iter()
            .filter(|s| s.gh_required && s.wave == 3)
            .map(|s| s.id.clone())
            .collect()
    };

    let mut results = Vec::new();
    for id in &selected_ids {
        let Some(meta) = by_id.get(id.as_str()) else {
            results.push(skip_result(
                id,
                3,
                &ctx,
                "scenario is not registered in yaml",
            ));
            continue;
        };
        if !meta.gh_required {
            results.push(skip_result(id, 3, &ctx, "not a gh_required live scenario"));
            continue;
        }
        if scenario_registry()
            .iter()
            .all(|(sid, _)| *sid != id.as_str())
        {
            results.push(skip_result(
                id,
                meta.wave,
                &ctx,
                "live scenario is not implemented in runner registry yet",
            ));
            continue;
        }
        // For live we do not early-skip on missing git here; the scenario fn or gitfix will surface.
        let maybe_fn = scenario_registry()
            .iter()
            .find(|(sid, _)| *sid == id.as_str())
            .map(|(_, f)| *f);
        results.push(if let Some(f) = maybe_fn {
            run_scenario(&ctx, id, meta.wave, f)
        } else {
            skip_result(
                id,
                meta.wave,
                &ctx,
                "live scenario is not implemented in runner yet",
            )
        });
    }

    write_report(&ctx, &results)?;

    // Post-run: if any live result has cleanup_required in its recorded status, surface it.
    let mut any_cleanup_required = false;
    for r in &results {
        if let Some(c) = &r.cleanup
            && c.contains("cleanup_required")
        {
            any_cleanup_required = true;
            eprintln!("[LIVE] {}", c);
        }
    }

    let failed = results.iter().filter(|r| r.status == "failed").count();
    let skipped = results.iter().filter(|r| r.status == "skipped").count();
    let passed = results.iter().filter(|r| r.status == "passed").count();
    println!("run_root={}", ctx.run_root.display());
    println!("passed={passed} failed={failed} skipped={skipped}");
    println!("report={}", ctx.run_root.join("report.json").display());
    if any_cleanup_required {
        eprintln!("WARNING: one or more Wave 3 repos require manual cleanup (see results)");
    }

    if failed == 0 && !keep {
        println!(
            "successful live run root kept for report inspection; pass --keep to make this explicit"
        );
    }
    if failed > 0 {
        bail!(
            "one or more live scenarios failed; run root kept at {}",
            ctx.run_root.display()
        );
    }
    Ok(())
}

fn write_live_skip_report(
    repo_root: &Path,
    only: Option<String>,
    binary: Option<PathBuf>,
    reason: &str,
    message: &str,
) -> Result<()> {
    let selected = parse_only(only);
    let mut results = Vec::new();
    let run_root = tempfile::Builder::new()
        .prefix("libra-integ-live-skipped-")
        .tempdir()?
        .keep();
    let dummy_ctx = RunContext {
        run_root: run_root.clone(),
        binary: binary.unwrap_or_else(|| repo_root.join("target/debug/libra")),
        safe_path: safe_path(),
        results_path: run_root.join("results.ndjson"),
    };
    for id in if selected.is_empty() {
        let m = load_manifest(repo_root)?;
        m.scenarios
            .iter()
            .filter(|s| s.gh_required)
            .map(|s| s.id.clone())
            .collect()
    } else {
        selected
    } {
        results.push(skip_result(&id, 3, &dummy_ctx, reason));
    }
    write_report(&dummy_ctx, &results)?;
    println!("run_root={}", dummy_ctx.run_root.display());
    println!("{message}");
    Ok(())
}
