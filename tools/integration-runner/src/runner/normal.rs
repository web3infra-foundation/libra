use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};

use super::{
    dispatch::{run_scenario, run_wave0, skip_result},
    types::RunContext,
    util::{ensure_binary, find_on_host_path, parse_only, parse_waves, safe_path},
};
use crate::{manifest::load_manifest, registry::scenario_registry, support::write_report};

pub(crate) fn run(
    repo_root: &Path,
    waves: Option<String>,
    only: Option<String>,
    binary: Option<PathBuf>,
    keep: bool,
) -> Result<()> {
    let manifest = load_manifest(repo_root)?;
    let selected_waves = parse_waves(waves)?;
    let selected_ids = parse_only(only);
    let binary = binary.unwrap_or_else(|| repo_root.join("target/debug/libra"));
    ensure_binary(repo_root, &binary)?;

    let run_root = tempfile::Builder::new()
        .prefix("libra-integ-")
        .tempdir()
        .context("create run root")?
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

    let mut results = Vec::new();
    if selected_waves.contains(&0) && selected_ids.is_empty() {
        results.push(run_wave0(&ctx));
    }

    let by_id: BTreeMap<_, _> = manifest
        .scenarios
        .iter()
        .map(|scenario| (scenario.id.as_str(), scenario))
        .collect();
    let scenario_ids = if selected_ids.is_empty() {
        manifest
            .scenarios
            .iter()
            .filter(|scenario| selected_waves.contains(&scenario.wave))
            .map(|scenario| scenario.id.as_str())
            .collect::<Vec<_>>()
    } else {
        selected_ids.iter().map(String::as_str).collect::<Vec<_>>()
    };

    for id in scenario_ids {
        let Some(meta) = by_id.get(id) else {
            results.push(skip_result(
                id,
                0,
                &ctx,
                "scenario is not registered in yaml",
            ));
            continue;
        };
        if scenario_registry().iter().all(|(sid, _)| *sid != id) {
            results.push(skip_result(
                id,
                meta.wave,
                &ctx,
                "scenario is not implemented in runner yet",
            ));
            continue;
        }
        if meta.gh_required {
            results.push(skip_result(
                id,
                meta.wave,
                &ctx,
                "gh_required live scenario; invoke via `run-live` (requires `gh auth`)",
            ));
            continue;
        }
        if meta.requires_git && find_on_host_path("git").is_none() {
            results.push(skip_result(
                id,
                meta.wave,
                &ctx,
                "scenario requires git, but git was not found on host PATH",
            ));
            continue;
        }

        // Dispatch via the single SCENARIO_REGISTRY (no per-scenario match arms).
        // Registration happens in one place: the array inside scenario_registry() in src/registry.rs.
        let maybe_fn = scenario_registry()
            .iter()
            .find(|(sid, _)| *sid == id)
            .map(|(_, f)| *f);
        results.push(if let Some(f) = maybe_fn {
            run_scenario(&ctx, id, meta.wave, f)
        } else {
            skip_result(
                id,
                meta.wave,
                &ctx,
                "scenario is not implemented in runner yet",
            )
        });
    }

    write_report(&ctx, &results)?;
    let failed = results.iter().filter(|r| r.status == "failed").count();
    let skipped = results.iter().filter(|r| r.status == "skipped").count();
    let passed = results.iter().filter(|r| r.status == "passed").count();
    println!("run_root={}", ctx.run_root.display());
    println!("passed={passed} failed={failed} skipped={skipped}");
    println!("report={}", ctx.run_root.join("report.json").display());

    if failed == 0 && !keep {
        println!(
            "successful run root kept for report inspection; pass --keep to make this explicit"
        );
    }
    if failed > 0 {
        bail!(
            "one or more scenarios failed; run root kept at {}",
            ctx.run_root.display()
        );
    }
    Ok(())
}
