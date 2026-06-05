use std::{fs, path::Path};

use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub(crate) struct ScenarioManifest {
    pub(crate) scenarios: Vec<ScenarioMeta>,
}

#[derive(Debug, Deserialize, Clone)]
pub(crate) struct ScenarioMeta {
    pub(crate) id: String,
    pub(crate) wave: u8,
    pub(crate) group: String,
    pub(crate) purpose: String,
    pub(crate) gh_required: bool,
    pub(crate) requires_git: bool,
    pub(crate) key_assertion_categories: Vec<String>,
    pub(crate) doc_section: String,
}

pub(crate) fn load_manifest(repo_root: &Path) -> Result<ScenarioManifest> {
    let path = repo_root.join("docs/development/integration-scenarios.yaml");
    let text = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    serde_yaml::from_str(&text).with_context(|| format!("parse {}", path.display()))
}

pub(crate) fn list(repo_root: &Path) -> Result<()> {
    let manifest = load_manifest(repo_root)?;
    for scenario in manifest.scenarios {
        println!(
            "wave={} id={} group={} gh_required={} requires_git={} purpose={}",
            scenario.wave,
            scenario.id,
            scenario.group,
            scenario.gh_required,
            scenario.requires_git,
            scenario.purpose
        );
    }
    Ok(())
}
