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

#[cfg(test)]
mod tests {
    use super::*;

    /// TOML without an `[mcp]` section produces an empty view —
    /// callers can blindly load and check `source()` without
    /// branching on file existence.
    #[test]
    fn source_config_view_from_toml_empty_input_yields_empty_view() {
        let view = source_config_view_from_toml("").expect("empty TOML must parse");
        assert!(view.sources().is_empty());
        assert!(view.source(BUILTIN_MCP_SOURCE_SLUG).is_none());
    }

    /// TOML without an `[mcp]` section (but with other content) also
    /// yields an empty view — the parser only cares about `[mcp]`.
    #[test]
    fn source_config_view_from_toml_unrelated_keys_yield_empty_view() {
        let view = source_config_view_from_toml("other_key = \"value\"\n[other_section]\nfoo = 1")
            .expect("valid TOML");
        assert!(view.sources().is_empty());
    }

    /// `[mcp]` table without an `enabled` field defaults to enabled
    /// (ProjectConfig enablement). Pin so a future "default to
    /// disabled" refactor breaks here — legacy users with `[mcp]`
    /// sections expect them to stay active.
    #[test]
    fn source_config_view_from_toml_mcp_table_defaults_to_enabled() {
        let view = source_config_view_from_toml("[mcp]\n").expect("valid TOML");
        let entry = view
            .source(BUILTIN_MCP_SOURCE_SLUG)
            .expect("builtin MCP slug must be present");
        assert_eq!(entry.enablement, SourceEnablement::ProjectConfig);
        assert_eq!(entry.kind, SourceKind::Mcp);
        assert_eq!(entry.origin, SourceConfigOrigin::LegacyMcp);
    }

    /// `[mcp] enabled = true` produces ProjectConfig enablement.
    #[test]
    fn source_config_view_from_toml_mcp_enabled_true_maps_to_project_config() {
        let view = source_config_view_from_toml("[mcp]\nenabled = true\n").expect("valid TOML");
        let entry = view.source(BUILTIN_MCP_SOURCE_SLUG).expect("present");
        assert_eq!(entry.enablement, SourceEnablement::ProjectConfig);
    }

    /// `[mcp] enabled = false` produces Disabled enablement. Pin this
    /// branch — operators rely on it to turn off MCP without removing
    /// the section.
    #[test]
    fn source_config_view_from_toml_mcp_enabled_false_maps_to_disabled() {
        let view = source_config_view_from_toml("[mcp]\nenabled = false\n").expect("valid TOML");
        let entry = view.source(BUILTIN_MCP_SOURCE_SLUG).expect("present");
        assert_eq!(entry.enablement, SourceEnablement::Disabled);
        // is_enabled() is false; is_explicit() is irrelevant for Disabled.
        assert!(!entry.enablement.is_enabled());
    }

    /// `mcp = true` (boolean form, not a table) maps to ProjectConfig.
    /// Pin the alternate shape so config-format simplifiers don't
    /// accidentally drop boolean support.
    #[test]
    fn source_config_view_from_toml_mcp_boolean_true_maps_to_project_config() {
        let view = source_config_view_from_toml("mcp = true\n").expect("valid TOML");
        let entry = view.source(BUILTIN_MCP_SOURCE_SLUG).expect("present");
        assert_eq!(entry.enablement, SourceEnablement::ProjectConfig);
    }

    /// `mcp = false` (boolean form) maps to Disabled.
    #[test]
    fn source_config_view_from_toml_mcp_boolean_false_maps_to_disabled() {
        let view = source_config_view_from_toml("mcp = false\n").expect("valid TOML");
        let entry = view.source(BUILTIN_MCP_SOURCE_SLUG).expect("present");
        assert_eq!(entry.enablement, SourceEnablement::Disabled);
    }

    /// Malformed TOML produces a parse error (NOT a silent default).
    /// Pin so the production code path that warns + falls back to
    /// `SourceConfigView::default()` is preceded by a recognisable
    /// error type.
    #[test]
    fn source_config_view_from_toml_malformed_returns_error() {
        let err = source_config_view_from_toml("[mcp\nenabled = ").unwrap_err();
        let rendered = err.to_string();
        assert!(
            !rendered.is_empty(),
            "TOML parser must surface a non-empty error",
        );
    }

    /// `SourceConfigView::source()` is a linear scan that finds the
    /// entry by slug. Pin the `Some`/`None` distinction.
    #[test]
    fn source_config_view_source_finds_entry_by_slug() {
        let view = source_config_view_from_toml("[mcp]\n").expect("valid TOML");
        assert!(view.source(BUILTIN_MCP_SOURCE_SLUG).is_some());
        assert!(view.source("nonexistent-slug").is_none());
    }

    /// `SourceConfigView::sources()` returns the live slice with
    /// entries in sorted order (the push helper sorts on insertion).
    #[test]
    fn source_config_view_sources_returns_sorted_slice() {
        // Today only the `[mcp]` source is ever pushed, but verifying
        // the slice shape pins the contract for future additions.
        let view = source_config_view_from_toml("[mcp]\nenabled = true\n").expect("valid");
        let slice = view.sources();
        assert_eq!(slice.len(), 1);
        // Slice is sorted; for a single entry, that's trivially true.
        assert_eq!(slice[0].slug, BUILTIN_MCP_SOURCE_SLUG);
    }

    /// Default `SourceConfigView` has zero sources — used by callers
    /// that haven't loaded a project config yet.
    #[test]
    fn source_config_view_default_is_empty() {
        let view = SourceConfigView::default();
        assert!(view.sources().is_empty());
    }
}
