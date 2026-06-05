use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use regex::Regex;

use crate::{manifest::load_manifest, registry::scenario_registry};

/// Per-scenario markdown under `docs/development/integration-scenarios/<id>.md`.
fn load_scenario_docs(repo_root: &Path) -> Result<(BTreeSet<String>, BTreeSet<String>)> {
    let scenarios_dir = repo_root.join("docs/development/integration-scenarios");
    let heading_re = Regex::new(r"(?m)^### `([^`]+)`").context("compile heading regex")?;
    let scenario_re = Regex::new(r#"SCENARIO="([^"]+)""#).context("compile scenario regex")?;
    let mut md_headings = BTreeSet::new();
    let mut md_scenarios = BTreeSet::new();
    for entry in
        fs::read_dir(&scenarios_dir).with_context(|| format!("read {}", scenarios_dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("md") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if !stem.starts_with("cli.") && !stem.starts_with("live.") {
            continue;
        }
        let body = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
        for cap in heading_re.captures_iter(&body) {
            md_headings.insert(cap[1].to_string());
        }
        for cap in scenario_re.captures_iter(&body) {
            let id = cap[1].to_string();
            if id != "cli.example-unique-id" {
                md_scenarios.insert(id);
            }
        }
        if !md_headings.contains(stem) {
            bail!(
                "scenario file {} must start with heading ### `{stem}`",
                path.display()
            );
        }
        if !md_scenarios.contains(stem) {
            bail!(
                "scenario file {} must contain SCENARIO=\"{stem}\"",
                path.display()
            );
        }
    }

    Ok((md_headings, md_scenarios))
}

pub(crate) fn check_plan(repo_root: &Path) -> Result<()> {
    let manifest = load_manifest(repo_root)?;
    let plan_path = repo_root.join("docs/development/integration-test-plan.md");
    let plan_md =
        fs::read_to_string(&plan_path).with_context(|| format!("read {}", plan_path.display()))?;
    let (md_headings, md_scenarios) = load_scenario_docs(repo_root)?;
    let yaml_ids: BTreeSet<_> = manifest.scenarios.iter().map(|s| s.id.as_str()).collect();
    let implemented: BTreeSet<_> = scenario_registry().iter().map(|(id, _)| *id).collect();

    let matrix_refs = extract_matrix_refs(&plan_md, &yaml_ids)?;
    let mut failures = Vec::new();

    // Further convergence gate: all Rust-implemented scenarios (from registry) must have their
    // MD ### section using the short/converged form (no full "libra() {" wrapper, or explicit
    // note that it references the single prelude in §3.3.1 / "手动执行 prelude").
    // This guarantees that when Agents implement Rust for a scenario, they also converge the
    // corresponding MD documentation.
    let converge_note_re = Regex::new(r"Short converged|Short form|Converged short|prelude.*top|converged short form|# \(prelude|Short converged form").context("compile converge note re")?;
    for id in &implemented {
        let path = repo_root.join(format!("docs/development/integration-scenarios/{id}.md"));
        if !path.is_file() {
            failures.push(format!(
                "Rust-implemented scenario {id} has no docs/development/integration-scenarios/{id}.md"
            ));
            continue;
        }
        let sec = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
        if sec.contains("libra() {") && !converge_note_re.is_match(&sec) {
            failures.push(format!(
                "Rust-implemented scenario {id} MD still contains long libra() wrapper without convergence note (must use short form per §0 checklist and §3.3.1)"
            ));
        }
    }

    for id in &yaml_ids {
        let path = PathBuf::from(format!("docs/development/integration-scenarios/{id}.md"));
        if !repo_root.join(&path).is_file() {
            failures.push(format!(
                "yaml scenario {id} has no matching file {}",
                path.display()
            ));
        }
    }

    for scenario in &manifest.scenarios {
        if scenario.doc_section != scenario.id {
            failures.push(format!(
                "yaml id {} has mismatched doc_section {}",
                scenario.id, scenario.doc_section
            ));
        }
        if scenario.gh_required && scenario.wave != 3 {
            failures.push(format!(
                "gh_required scenario {} is not Wave 3",
                scenario.id
            ));
        }
        if scenario.wave == 3
            && scenario.gh_required
            && !scenario
                .key_assertion_categories
                .iter()
                .any(|category| category == "gh_lifecycle")
        {
            failures.push(format!(
                "Wave 3 scenario {} lacks gh_lifecycle",
                scenario.id
            ));
        }
        if !md_headings.contains(&scenario.id) {
            failures.push(format!(
                "yaml scenario {} has no matching MD heading",
                scenario.id
            ));
        }
        if !md_scenarios.contains(&scenario.id) {
            failures.push(format!(
                "yaml scenario {} has no matching SCENARIO= block",
                scenario.id
            ));
        }
    }

    for id in &md_headings {
        if !yaml_ids.contains(id.as_str()) {
            failures.push(format!("MD heading {id} is not registered in yaml"));
        }
    }
    for id in &md_scenarios {
        if !yaml_ids.contains(id.as_str()) {
            failures.push(format!("MD SCENARIO={id} is not registered in yaml"));
        }
    }
    for id in &matrix_refs {
        if !yaml_ids.contains(id.as_str()) {
            failures.push(format!("§2.3 matrix references unregistered scenario {id}"));
        }
    }
    for id in &implemented {
        if !yaml_ids.contains(id) {
            failures.push(format!(
                "runner implements {id}, but yaml does not register it"
            ));
        }
    }

    let not_implemented: Vec<_> = yaml_ids
        .iter()
        .filter(|id| !implemented.contains(**id))
        .copied()
        .collect();

    println!("yaml_scenarios={}", yaml_ids.len());
    println!("scenario_doc_files={}", md_headings.len());
    println!("md_scenario_blocks={}", md_scenarios.len());
    println!("matrix_refs={}", matrix_refs.len());
    println!("implemented={}", implemented.len());
    println!("documented_but_not_implemented={}", not_implemented.len());
    for id in not_implemented {
        println!("not_implemented {id}");
    }

    if !failures.is_empty() {
        for failure in failures {
            eprintln!("check-plan failure: {failure}");
        }
        bail!("check-plan failed");
    }

    Ok(())
}

fn extract_matrix_refs(md: &str, yaml_ids: &BTreeSet<&str>) -> Result<BTreeSet<String>> {
    let start = md
        .find("### 2.3 ")
        .context("missing §2.3 command coverage matrix")?;
    let end = md[start..]
        .find("**剩余覆盖缺口")
        .map(|offset| start + offset)
        .context("missing end of §2.3 matrix")?;
    let matrix = &md[start..end];
    let id_re = Regex::new(r"\b(?:cli|live)\.[A-Za-z0-9_.-]+\*?").context("compile id regex")?;
    let mut refs = BTreeSet::new();
    for line in matrix
        .lines()
        .filter(|line| line.trim_start().starts_with('|'))
    {
        let cells: Vec<_> = line.split('|').map(str::trim).collect();
        let Some(main_ids_cell) = cells.get(5) else {
            continue;
        };
        for matched in id_re.find_iter(main_ids_cell).map(|m| m.as_str()) {
            if let Some(prefix) = matched.strip_suffix('*') {
                if !yaml_ids.iter().any(|id| id.starts_with(prefix)) {
                    refs.insert(matched.to_string());
                }
            } else {
                refs.insert(matched.trim_end_matches('.').to_string());
            }
        }
    }
    Ok(refs)
}
