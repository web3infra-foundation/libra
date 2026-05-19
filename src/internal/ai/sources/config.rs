//! Source configuration view and legacy MCP config compatibility.
//!
//! Phase A keeps the built-in MCP bridge behavior intact while exposing the
//! same capability through Source Pool. If a project still has a legacy
//! `.libra/config.toml` `[mcp]` section, map it to the equivalent built-in MCP
//! source view and warn so users can migrate later.

use std::{fs, path::Path, sync::Arc};

use serde::Deserialize;

use super::{
    BUILTIN_MCP_SOURCE_SLUG, McpSource, SourceEnablement, SourceKind, SourcePool, SourcePoolError,
};
use crate::internal::ai::mcp::server::LibraMcpServer;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceConfigOrigin {
    LegacyMcp,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceConfigEntry {
    pub slug: String,
    pub kind: SourceKind,
    pub enablement: SourceEnablement,
    pub origin: SourceConfigOrigin,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SourceConfigView {
    sources: Vec<SourceConfigEntry>,
}

impl SourceConfigView {
    pub fn source(&self, slug: &str) -> Option<&SourceConfigEntry> {
        self.sources.iter().find(|source| source.slug == slug)
    }

    pub fn sources(&self) -> &[SourceConfigEntry] {
        &self.sources
    }

    fn push(&mut self, entry: SourceConfigEntry) {
        self.sources.push(entry);
        self.sources
            .sort_by(|left, right| left.slug.cmp(&right.slug));
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SourceConfigLoadReport {
    pub legacy_mcp_config_mapped: bool,
}

#[derive(Debug, Deserialize)]
struct ProjectSourceConfig {
    #[serde(default)]
    mcp: Option<toml::Value>,
}

pub fn source_config_view_from_project_config(working_dir: &Path) -> SourceConfigView {
    let path = working_dir.join(".libra").join("config.toml");
    let Ok(contents) = fs::read_to_string(&path) else {
        return SourceConfigView::default();
    };

    let view = match source_config_view_from_toml(&contents) {
        Ok(view) => view,
        Err(error) => {
            tracing::warn!(
                target: "libra::ai::sources::config",
                path = %path.display(),
                error = %error,
                "failed to parse source config; using default source view"
            );
            SourceConfigView::default()
        }
    };

    if view.source(BUILTIN_MCP_SOURCE_SLUG).is_some() {
        tracing::warn!(
            target: "libra::ai::sources::config",
            path = %path.display(),
            source = BUILTIN_MCP_SOURCE_SLUG,
            "`[mcp]` project config is deprecated; treating it as the built-in MCP source view"
        );
    }

    view
}

fn source_config_view_from_toml(contents: &str) -> Result<SourceConfigView, toml::de::Error> {
    let config: ProjectSourceConfig = toml::from_str(contents)?;
    let mut view = SourceConfigView::default();
    if let Some(mcp) = config.mcp.as_ref() {
        let enablement = if legacy_mcp_config_enabled(mcp) {
            SourceEnablement::ProjectConfig
        } else {
            SourceEnablement::Disabled
        };
        view.push(SourceConfigEntry {
            slug: BUILTIN_MCP_SOURCE_SLUG.to_string(),
            kind: SourceKind::Mcp,
            enablement,
            origin: SourceConfigOrigin::LegacyMcp,
        });
    }
    Ok(view)
}

fn legacy_mcp_config_enabled(value: &toml::Value) -> bool {
    match value {
        toml::Value::Boolean(enabled) => *enabled,
        toml::Value::Table(table) => table
            .get("enabled")
            .and_then(toml::Value::as_bool)
            .unwrap_or(true),
        _ => true,
    }
}

pub fn register_builtin_mcp_source_from_project_config(
    source_pool: &SourcePool,
    server: Arc<LibraMcpServer>,
    working_dir: &Path,
) -> Result<SourceConfigLoadReport, SourcePoolError> {
    let view = source_config_view_from_project_config(working_dir);
    let legacy_mcp_config_mapped = view.source(BUILTIN_MCP_SOURCE_SLUG).is_some();
    let enablement = view
        .source(BUILTIN_MCP_SOURCE_SLUG)
        .map(|entry| entry.enablement)
        .unwrap_or(SourceEnablement::Builtin);
    source_pool
        .register_source_with_enablement(Arc::new(McpSource::builtin(server)), enablement)?;
    Ok(SourceConfigLoadReport {
        legacy_mcp_config_mapped,
    })
}
