//! Hook providers for the five promoted external agents
//! (Cursor / Codex / Copilot CLI / Factory AI Droid / OpenCode).
//!
//! Unlike the bespoke Claude and Gemini providers, all five share one
//! [`PromotedProvider`] type driven by a static [`PromotedSpec`]. The spec
//! selects one of three install backends matching the agent's native config
//! format ([`HookShape`]):
//!
//! - **Claude-matcher** (`.codex/hooks.json`, `.factory/settings.json`):
//!   `{"hooks":{"<Event>":[{"matcher":…,"hooks":[{"type":"command","command":…}]}]}}`.
//! - **Flat cursor-style** (`.cursor/hooks.json`, `.github/hooks/libra.json`):
//!   `{"version":1,"hooks":{"<event>":[{ <cmd-field>: "…" }]}}`.
//! - **TS plugin** (`.opencode/plugins/libra.ts`): a generated TypeScript
//!   plugin that shells out to `libra agent hooks opencode <subcommand>`.
//!
//! Config shapes are ported from EntireIO's `cmd/entire/cli/agent/<a>/hooks.go`
//! (the verified reference). The installed command is
//! `<libra> agent hooks <slug> <subcommand> || true`; the `|| true` fail-safe
//! (entire.md §6.4) guarantees a Libra hiccup never breaks the host agent.
//!
//! Because Libra *authors* each config, [`HookProvider::subcommand_is_authoritative`]
//! returns `true` for these providers: the AgentTraces ingest derives the
//! lifecycle kind from the installed subcommand rather than the agents'
//! heterogeneous (OpenCode: absent) stdin `hook_event_name`. [`parse_promoted_hook_event`]
//! still maps a comprehensive alias set for completeness.

use std::collections::BTreeMap;

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::super::{
    lifecycle::{LifecycleEvent, LifecycleEventKind, SessionHookEnvelope, build_lifecycle_event},
    provider::{
        CANONICAL_DEDUP_IDENTITY_KEYS, HookProvider, ProviderHookCommand, ProviderInstallOptions,
    },
};
use crate::internal::ai::hooks::setup::{
    load_json_settings, resolve_hook_binary_path, resolve_project_root, write_json_settings,
};

const DEFAULT_HOOK_TIMEOUT_SECS: u64 = 30;

/// Native config shape of a promoted agent's hook file.
#[derive(Debug, Clone, Copy)]
pub enum HookShape {
    /// Claude-matcher group shape. `matcher` is serialised verbatim (`None`
    /// → JSON `null`, `Some("")` → `""`) to match the agent's expectation.
    ClaudeMatcher {
        matcher: Option<&'static str>,
        timeout: Option<u64>,
    },
    /// Flat per-event arrays of command objects. `command_field` is the JSON
    /// key holding the shell string (`"command"` for Cursor, `"bash"` for
    /// Copilot); `with_type` adds `"type":"command"`; `comment` adds an
    /// optional marker comment.
    Flat {
        command_field: &'static str,
        with_type: bool,
        comment: Option<&'static str>,
    },
    /// A generated TypeScript plugin file.
    TsPlugin,
}

/// Static description of one promoted agent's hook wiring.
pub struct PromotedSpec {
    pub slug: &'static str,
    pub source_name: &'static str,
    /// Config path relative to the worktree/project root.
    pub rel_path: &'static str,
    pub shape: HookShape,
    /// `(config event key, libra subcommand)` pairs. The subcommand must be a
    /// valid `libra agent hooks <slug>` kebab subcommand
    /// (`session-start`/`prompt`/`tool-use`/`stop`/`session-end`/`compaction`).
    pub forward: &'static [(&'static str, &'static str)],
    pub supported: &'static [ProviderHookCommand],
}

/// Generic provider over a [`PromotedSpec`].
pub struct PromotedProvider {
    spec: &'static PromotedSpec,
}

impl HookProvider for PromotedProvider {
    fn provider_name(&self) -> &'static str {
        self.spec.slug
    }

    fn source_name(&self) -> &'static str {
        self.spec.source_name
    }

    fn supported_commands(&self) -> &'static [ProviderHookCommand] {
        self.spec.supported
    }

    fn parse_hook_event(
        &self,
        hook_event_name: &str,
        envelope: &SessionHookEnvelope,
    ) -> Result<LifecycleEvent> {
        parse_promoted_hook_event(hook_event_name, envelope)
    }

    fn dedup_identity_keys(&self) -> &'static [&'static str] {
        CANONICAL_DEDUP_IDENTITY_KEYS
    }

    fn lifecycle_fallback_events(&self) -> &'static [&'static str] {
        &[]
    }

    fn install_hooks(&self, options: &ProviderInstallOptions) -> Result<()> {
        let binary_path = resolve_hook_binary_path(options.binary_path.as_deref())?;
        let prefix = options
            .hook_command_prefix
            .clone()
            .unwrap_or_else(|| format!("agent hooks {}", self.spec.slug));
        match self.spec.shape {
            HookShape::TsPlugin => install_ts_plugin(self.spec, &binary_path, &prefix),
            _ => install_json_hooks(self.spec, &binary_path, &prefix, options),
        }
    }

    fn uninstall_hooks(&self) -> Result<()> {
        match self.spec.shape {
            HookShape::TsPlugin => uninstall_ts_plugin(self.spec),
            _ => uninstall_json_hooks(self.spec),
        }
    }

    fn hooks_are_installed(&self) -> Result<bool> {
        match self.spec.shape {
            HookShape::TsPlugin => ts_plugin_installed(self.spec),
            _ => json_hooks_installed(self.spec),
        }
    }

    fn subcommand_is_authoritative(&self) -> bool {
        true
    }
}

/// Build the installed shell command `<binary> <prefix> <subcommand>[ || true]`.
fn build_command(binary_path: &str, prefix: &str, subcommand: &str, fail_safe: bool) -> String {
    let command = format!("{binary_path} {prefix} {subcommand}");
    if fail_safe {
        format!("{command} || true")
    } else {
        command
    }
}

/// A command string is Libra-managed if (ignoring a trailing `|| true`) it
/// contains the ` agent hooks <slug> ` marker this module installs. Robust
/// across all shapes / wrappers without parsing the exact binary path.
fn command_is_managed(command: &str, slug: &str) -> bool {
    let marker = format!(" agent hooks {slug} ");
    command.contains(&marker)
}

// ---------------------------------------------------------------------------
// JSON config backends (Claude-matcher + Flat)
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Serialize, Deserialize)]
struct MatcherFile {
    #[serde(default)]
    hooks: BTreeMap<String, Vec<MatcherGroup>>,
    #[serde(flatten)]
    extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct MatcherGroup {
    // No skip_serializing_if: `None` must serialise as JSON `null` (Codex),
    // `Some("")` as `""` (Factory) — both mean "match all".
    matcher: Option<String>,
    hooks: Vec<MatcherEntry>,
    #[serde(flatten)]
    extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct MatcherEntry {
    #[serde(rename = "type")]
    entry_type: String,
    command: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    timeout: Option<u64>,
    #[serde(flatten)]
    extra: BTreeMap<String, Value>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct FlatFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    version: Option<u32>,
    #[serde(default)]
    hooks: BTreeMap<String, Vec<Value>>,
    #[serde(flatten)]
    extra: BTreeMap<String, Value>,
}

fn config_path(spec: &PromotedSpec) -> Result<std::path::PathBuf> {
    Ok(resolve_project_root()?.join(spec.rel_path))
}

fn install_json_hooks(
    spec: &PromotedSpec,
    binary_path: &str,
    prefix: &str,
    options: &ProviderInstallOptions,
) -> Result<()> {
    let path = config_path(spec)?;
    let changed = match spec.shape {
        HookShape::ClaudeMatcher { matcher, timeout } => {
            let mut file: MatcherFile = load_json_settings(&path, spec.slug)?;
            let timeout = timeout.or(options.timeout_secs);
            let changed = upsert_matcher(
                &mut file,
                spec,
                binary_path,
                prefix,
                matcher,
                timeout,
                options.fail_safe_shell,
            );
            if changed {
                write_json_settings(&path, &file, spec.slug)?;
            }
            changed
        }
        HookShape::Flat {
            command_field,
            with_type,
            comment,
        } => {
            let mut file: FlatFile = load_json_settings(&path, spec.slug)?;
            if file.version.is_none() {
                file.version = Some(1);
            }
            let changed = upsert_flat(
                &mut file,
                spec,
                binary_path,
                prefix,
                command_field,
                with_type,
                comment,
                options.fail_safe_shell,
            );
            if changed {
                write_json_settings(&path, &file, spec.slug)?;
            }
            changed
        }
        HookShape::TsPlugin => unreachable!("ts plugin handled separately"),
    };
    if changed {
        println!(
            "Installed {} hook forwarding at {}",
            spec.slug,
            path.display()
        );
    } else {
        println!(
            "{} hook forwarding is already up to date at {}",
            spec.slug,
            path.display()
        );
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn upsert_matcher(
    file: &mut MatcherFile,
    spec: &PromotedSpec,
    binary_path: &str,
    prefix: &str,
    matcher: Option<&str>,
    timeout: Option<u64>,
    fail_safe: bool,
) -> bool {
    let mut changed = false;
    for (event_key, subcommand) in spec.forward {
        let desired = MatcherEntry {
            entry_type: "command".to_string(),
            command: build_command(binary_path, prefix, subcommand, fail_safe),
            timeout,
            extra: BTreeMap::new(),
        };
        let groups = file.hooks.entry((*event_key).to_string()).or_default();
        let original = groups.clone();
        // Strip any stale Libra-managed entry for this slug from all groups,
        // drop emptied groups, then append the desired entry in its own
        // match-all group. Comparing to `original` keeps a re-install a no-op.
        for group in groups.iter_mut() {
            group
                .hooks
                .retain(|h| !command_is_managed(&h.command, spec.slug));
        }
        groups.retain(|g| !g.hooks.is_empty());
        groups.push(MatcherGroup {
            matcher: matcher.map(str::to_string),
            hooks: vec![desired],
            extra: BTreeMap::new(),
        });
        if *groups != original {
            changed = true;
        }
    }
    changed
}

#[allow(clippy::too_many_arguments)]
fn upsert_flat(
    file: &mut FlatFile,
    spec: &PromotedSpec,
    binary_path: &str,
    prefix: &str,
    command_field: &str,
    with_type: bool,
    comment: Option<&str>,
    fail_safe: bool,
) -> bool {
    let mut changed = false;
    for (event_key, subcommand) in spec.forward {
        let command = build_command(binary_path, prefix, subcommand, fail_safe);
        let entries = file.hooks.entry((*event_key).to_string()).or_default();
        let original = entries.clone();
        entries.retain(|e| !flat_entry_is_managed(e, command_field, spec.slug));
        let mut obj = Map::new();
        if with_type {
            obj.insert("type".to_string(), Value::String("command".to_string()));
        }
        obj.insert(command_field.to_string(), Value::String(command));
        if let Some(c) = comment {
            obj.insert("comment".to_string(), Value::String(c.to_string()));
        }
        entries.push(Value::Object(obj));
        if *entries != original {
            changed = true;
        }
    }
    changed
}

fn flat_entry_is_managed(entry: &Value, command_field: &str, slug: &str) -> bool {
    entry
        .get(command_field)
        .and_then(Value::as_str)
        .is_some_and(|cmd| command_is_managed(cmd, slug))
}

fn uninstall_json_hooks(spec: &PromotedSpec) -> Result<()> {
    let path = config_path(spec)?;
    if !path.exists() {
        println!(
            "{} hook settings not found at {}",
            spec.slug,
            path.display()
        );
        return Ok(());
    }
    let changed = match spec.shape {
        HookShape::ClaudeMatcher { .. } => {
            let mut file: MatcherFile = load_json_settings(&path, spec.slug)?;
            let mut changed = false;
            let keys: Vec<String> = file.hooks.keys().cloned().collect();
            for key in keys {
                let Some(mut groups) = file.hooks.remove(&key) else {
                    continue;
                };
                let original = groups.clone();
                for group in &mut groups {
                    group
                        .hooks
                        .retain(|h| !command_is_managed(&h.command, spec.slug));
                }
                groups.retain(|g| !g.hooks.is_empty());
                if groups != original {
                    changed = true;
                }
                if !groups.is_empty() {
                    file.hooks.insert(key, groups);
                }
            }
            if changed {
                write_json_settings(&path, &file, spec.slug)?;
            }
            changed
        }
        HookShape::Flat { command_field, .. } => {
            let mut file: FlatFile = load_json_settings(&path, spec.slug)?;
            let mut changed = false;
            let keys: Vec<String> = file.hooks.keys().cloned().collect();
            for key in keys {
                let Some(mut entries) = file.hooks.remove(&key) else {
                    continue;
                };
                let before = entries.len();
                entries.retain(|e| !flat_entry_is_managed(e, command_field, spec.slug));
                if entries.len() != before {
                    changed = true;
                }
                if !entries.is_empty() {
                    file.hooks.insert(key, entries);
                }
            }
            if changed {
                write_json_settings(&path, &file, spec.slug)?;
            }
            changed
        }
        HookShape::TsPlugin => unreachable!(),
    };
    if changed {
        println!(
            "Removed {} hook forwarding at {}",
            spec.slug,
            path.display()
        );
    } else {
        println!(
            "No Libra-managed {} hooks found at {}",
            spec.slug,
            path.display()
        );
    }
    Ok(())
}

fn json_hooks_installed(spec: &PromotedSpec) -> Result<bool> {
    let path = config_path(spec)?;
    if !path.exists() {
        return Ok(false);
    }
    match spec.shape {
        HookShape::ClaudeMatcher { .. } => {
            let file: MatcherFile = load_json_settings(&path, spec.slug)?;
            Ok(spec.forward.iter().all(|(event_key, _)| {
                file.hooks.get(*event_key).is_some_and(|groups| {
                    groups.iter().any(|g| {
                        g.hooks
                            .iter()
                            .any(|h| command_is_managed(&h.command, spec.slug))
                    })
                })
            }))
        }
        HookShape::Flat { command_field, .. } => {
            let file: FlatFile = load_json_settings(&path, spec.slug)?;
            Ok(spec.forward.iter().all(|(event_key, _)| {
                file.hooks.get(*event_key).is_some_and(|entries| {
                    entries
                        .iter()
                        .any(|e| flat_entry_is_managed(e, command_field, spec.slug))
                })
            }))
        }
        HookShape::TsPlugin => unreachable!(),
    }
}

// ---------------------------------------------------------------------------
// TypeScript plugin backend (OpenCode)
// ---------------------------------------------------------------------------

/// Marker embedded in the generated plugin so uninstall/status can recognise
/// a Libra-owned file without parsing TypeScript.
const TS_MARKER: &str = "libra-agent-capture-plugin";

fn render_ts_plugin(binary_path: &str, prefix: &str) -> String {
    // Each OpenCode event spawns `<binary> <prefix> <subcommand>` with a small
    // JSON payload (session_id) on stdin. `{:?}` renders each command as a
    // quoted, escaped JS string literal. Failures are swallowed so the plugin
    // can never break the host agent.
    let c = |sub: &str| format!("{binary_path} {prefix} {sub}");
    format!(
        r#"// {marker}
// Auto-generated by `libra agent enable opencode`. Forwards OpenCode session
// lifecycle events to Libra's agent-traces capture. Delete to disable.
export const event = async ({{ event, directory }}) => {{
  const run = async (cmd, id) => {{
    if (!id) return;
    try {{
      const proc = Bun.spawn(["sh", "-c", cmd], {{
        cwd: directory,
        stdin: new Blob([JSON.stringify({{ session_id: id }}) + "\n"]),
        stdout: "ignore",
        stderr: "ignore",
      }});
      await proc.exited;
    }} catch (_e) {{ /* never break the host agent */ }}
  }};
  const p = event.properties ?? {{}};
  switch (event.type) {{
    case "session.created": await run({start:?}, p.info?.id); break;
    case "message.part.updated": await run({prompt:?}, p.part?.sessionID); break;
    case "session.idle": await run({stop:?}, p.sessionID ?? p.info?.id); break;
    case "session.compacted": await run({compaction:?}, p.sessionID ?? p.info?.id); break;
    case "session.deleted": await run({end:?}, p.info?.id); break;
  }}
}};
"#,
        marker = TS_MARKER,
        start = c("session-start"),
        prompt = c("prompt"),
        stop = c("stop"),
        compaction = c("compaction"),
        end = c("session-end"),
    )
}

fn install_ts_plugin(spec: &PromotedSpec, binary_path: &str, prefix: &str) -> Result<()> {
    let path = config_path(spec)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| anyhow::anyhow!("failed to create {} plugin dir: {e}", spec.slug))?;
    }
    std::fs::write(&path, render_ts_plugin(binary_path, prefix))
        .map_err(|e| anyhow::anyhow!("failed to write {} plugin: {e}", spec.slug))?;
    println!(
        "Installed {} capture plugin at {}",
        spec.slug,
        path.display()
    );
    Ok(())
}

fn uninstall_ts_plugin(spec: &PromotedSpec) -> Result<()> {
    let path = config_path(spec)?;
    if !path.exists() {
        println!(
            "{} capture plugin not found at {}",
            spec.slug,
            path.display()
        );
        return Ok(());
    }
    // Only remove a file we own (carries our marker) so a user's hand-written
    // plugin at the same path is never deleted.
    let body = std::fs::read_to_string(&path).unwrap_or_default();
    if !body.contains(TS_MARKER) {
        println!(
            "{} plugin at {} is not Libra-managed; leaving it untouched",
            spec.slug,
            path.display()
        );
        return Ok(());
    }
    std::fs::remove_file(&path)
        .map_err(|e| anyhow::anyhow!("failed to remove {} plugin: {e}", spec.slug))?;
    println!("Removed {} capture plugin at {}", spec.slug, path.display());
    Ok(())
}

fn ts_plugin_installed(spec: &PromotedSpec) -> Result<bool> {
    let path = config_path(spec)?;
    if !path.exists() {
        return Ok(false);
    }
    Ok(std::fs::read_to_string(&path)
        .map(|b| b.contains(TS_MARKER))
        .unwrap_or(false))
}

// ---------------------------------------------------------------------------
// Shared event-name → lifecycle-kind alias parser
// ---------------------------------------------------------------------------

/// Map any of the promoted agents' hook event names — native PascalCase,
/// camelCase, kebab-case, or OpenCode dotted forms — onto a
/// [`LifecycleEventKind`]. The ingest path trusts the subcommand instead (see
/// [`HookProvider::subcommand_is_authoritative`]); this exists for completeness
/// and for any non-ingest caller.
pub fn parse_promoted_hook_event(
    hook_event_name: &str,
    envelope: &SessionHookEnvelope,
) -> Result<LifecycleEvent> {
    let normalized: String = hook_event_name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect();
    let kind = match normalized.as_str() {
        "sessionstart" | "sessioncreated" => LifecycleEventKind::SessionStart,
        "userpromptsubmit"
        | "userpromptsubmitted"
        | "beforesubmitprompt"
        | "prompt"
        | "messagepartupdated" => LifecycleEventKind::TurnStart,
        "posttooluse" | "pretooluse" | "tooluse" => LifecycleEventKind::ToolUse,
        "stop" | "agentstop" | "sessionstop" | "sessionidle" => LifecycleEventKind::TurnEnd,
        "sessionend" | "sessiondeleted" => LifecycleEventKind::SessionEnd,
        "compaction" | "precompact" | "sessioncompacted" => LifecycleEventKind::Compaction,
        "modelupdate" => LifecycleEventKind::ModelUpdate,
        other => bail!("unknown promoted-agent hook event: '{other}'"),
    };
    Ok(build_lifecycle_event(kind, envelope))
}

// ---------------------------------------------------------------------------
// The five static specs + providers
// ---------------------------------------------------------------------------

const CURSOR_SUPPORTED: &[ProviderHookCommand] = &[
    ProviderHookCommand::SessionStart,
    ProviderHookCommand::Prompt,
    ProviderHookCommand::Stop,
    ProviderHookCommand::Compaction,
    ProviderHookCommand::SessionEnd,
];

static CURSOR_SPEC: PromotedSpec = PromotedSpec {
    slug: "cursor",
    source_name: "cursor_hook",
    rel_path: ".cursor/hooks.json",
    shape: HookShape::Flat {
        command_field: "command",
        with_type: false,
        comment: None,
    },
    forward: &[
        ("sessionStart", "session-start"),
        ("beforeSubmitPrompt", "prompt"),
        ("stop", "stop"),
        ("preCompact", "compaction"),
        ("sessionEnd", "session-end"),
    ],
    supported: CURSOR_SUPPORTED,
};
pub static CURSOR_PROVIDER: PromotedProvider = PromotedProvider { spec: &CURSOR_SPEC };

const CODEX_SUPPORTED: &[ProviderHookCommand] = &[
    ProviderHookCommand::SessionStart,
    ProviderHookCommand::Prompt,
    ProviderHookCommand::ToolUse,
    ProviderHookCommand::Stop,
];

static CODEX_SPEC: PromotedSpec = PromotedSpec {
    slug: "codex",
    source_name: "codex_hook",
    rel_path: ".codex/hooks.json",
    shape: HookShape::ClaudeMatcher {
        matcher: None,
        timeout: Some(DEFAULT_HOOK_TIMEOUT_SECS),
    },
    forward: &[
        ("SessionStart", "session-start"),
        ("UserPromptSubmit", "prompt"),
        ("PostToolUse", "tool-use"),
        ("Stop", "stop"),
    ],
    supported: CODEX_SUPPORTED,
};
pub static CODEX_PROVIDER: PromotedProvider = PromotedProvider { spec: &CODEX_SPEC };

const COPILOT_SUPPORTED: &[ProviderHookCommand] = &[
    ProviderHookCommand::SessionStart,
    ProviderHookCommand::Prompt,
    ProviderHookCommand::ToolUse,
    ProviderHookCommand::Stop,
    ProviderHookCommand::SessionEnd,
];

static COPILOT_SPEC: PromotedSpec = PromotedSpec {
    slug: "copilot",
    source_name: "copilot_cli_hook",
    rel_path: ".github/hooks/libra.json",
    shape: HookShape::Flat {
        command_field: "bash",
        with_type: true,
        comment: Some("Libra agent capture"),
    },
    forward: &[
        ("sessionStart", "session-start"),
        ("userPromptSubmitted", "prompt"),
        ("preToolUse", "tool-use"),
        ("postToolUse", "tool-use"),
        ("agentStop", "stop"),
        ("sessionEnd", "session-end"),
    ],
    supported: COPILOT_SUPPORTED,
};
pub static COPILOT_PROVIDER: PromotedProvider = PromotedProvider {
    spec: &COPILOT_SPEC,
};

const FACTORY_SUPPORTED: &[ProviderHookCommand] = &[
    ProviderHookCommand::SessionStart,
    ProviderHookCommand::Prompt,
    ProviderHookCommand::ToolUse,
    ProviderHookCommand::Compaction,
    ProviderHookCommand::Stop,
    ProviderHookCommand::SessionEnd,
];

static FACTORY_SPEC: PromotedSpec = PromotedSpec {
    slug: "factory-ai",
    source_name: "factory_ai_droid_hook",
    rel_path: ".factory/settings.json",
    shape: HookShape::ClaudeMatcher {
        matcher: Some(""),
        timeout: None,
    },
    forward: &[
        ("SessionStart", "session-start"),
        ("UserPromptSubmit", "prompt"),
        ("PreToolUse", "tool-use"),
        ("PostToolUse", "tool-use"),
        ("PreCompact", "compaction"),
        ("Stop", "stop"),
        ("SessionEnd", "session-end"),
    ],
    supported: FACTORY_SUPPORTED,
};
pub static FACTORY_PROVIDER: PromotedProvider = PromotedProvider {
    spec: &FACTORY_SPEC,
};

const OPENCODE_SUPPORTED: &[ProviderHookCommand] = &[
    ProviderHookCommand::SessionStart,
    ProviderHookCommand::Prompt,
    ProviderHookCommand::Compaction,
    ProviderHookCommand::Stop,
    ProviderHookCommand::SessionEnd,
];

static OPENCODE_SPEC: PromotedSpec = PromotedSpec {
    slug: "opencode",
    source_name: "opencode_hook",
    rel_path: ".opencode/plugins/libra.ts",
    shape: HookShape::TsPlugin,
    // Forward map is informational for TsPlugin (the template hard-codes the
    // event→subcommand routing) but kept for status/listing parity.
    forward: &[
        ("session.created", "session-start"),
        ("message.part.updated", "prompt"),
        ("session.idle", "stop"),
        ("session.compacted", "compaction"),
        ("session.deleted", "session-end"),
    ],
    supported: OPENCODE_SUPPORTED,
};
pub static OPENCODE_PROVIDER: PromotedProvider = PromotedProvider {
    spec: &OPENCODE_SPEC,
};

#[cfg(test)]
mod tests {
    use serde_json::Map;

    use super::*;

    const BIN: &str = "/usr/local/bin/libra";

    // ----- Claude-matcher backend (Codex / Factory) -----

    #[test]
    fn matcher_install_writes_command_under_each_event_key() {
        let mut file = MatcherFile::default();
        let changed = upsert_matcher(
            &mut file,
            &CODEX_SPEC,
            BIN,
            "agent hooks codex",
            None,
            Some(30),
            true,
        );
        assert!(changed);
        for (event_key, sub) in CODEX_SPEC.forward {
            let groups = file.hooks.get(*event_key).expect("event key present");
            let cmd = &groups[0].hooks[0].command;
            assert_eq!(
                cmd,
                &format!("{BIN} agent hooks codex {sub} || true"),
                "command for {event_key}"
            );
            assert_eq!(groups[0].hooks[0].timeout, Some(30));
            // Codex uses matcher: null.
            assert_eq!(groups[0].matcher, None);
        }
    }

    #[test]
    fn matcher_install_is_idempotent() {
        let mut file = MatcherFile::default();
        assert!(upsert_matcher(
            &mut file,
            &CODEX_SPEC,
            BIN,
            "agent hooks codex",
            None,
            Some(30),
            true
        ));
        assert!(
            !upsert_matcher(
                &mut file,
                &CODEX_SPEC,
                BIN,
                "agent hooks codex",
                None,
                Some(30),
                true
            ),
            "second install must be a no-op"
        );
    }

    #[test]
    fn factory_matcher_uses_empty_string_matcher() {
        let mut file = MatcherFile::default();
        upsert_matcher(
            &mut file,
            &FACTORY_SPEC,
            BIN,
            "agent hooks factory-ai",
            Some(""),
            None,
            true,
        );
        let groups = file.hooks.get("PreToolUse").expect("PreToolUse present");
        assert_eq!(groups[0].matcher.as_deref(), Some(""));
        assert_eq!(groups[0].hooks[0].timeout, None);
    }

    #[test]
    fn matcher_install_preserves_foreign_hooks_and_keys() {
        let mut file = MatcherFile::default();
        file.extra
            .insert("$schema".to_string(), Value::String("x".to_string()));
        file.hooks.insert(
            "SessionStart".to_string(),
            vec![MatcherGroup {
                matcher: None,
                hooks: vec![MatcherEntry {
                    entry_type: "command".to_string(),
                    command: "/other/tool run".to_string(),
                    timeout: None,
                    extra: BTreeMap::new(),
                }],
                extra: BTreeMap::new(),
            }],
        );
        upsert_matcher(
            &mut file,
            &CODEX_SPEC,
            BIN,
            "agent hooks codex",
            None,
            Some(30),
            true,
        );
        assert!(
            file.extra.contains_key("$schema"),
            "unknown top-level key preserved"
        );
        let cmds: Vec<&str> = file.hooks["SessionStart"]
            .iter()
            .flat_map(|g| g.hooks.iter())
            .map(|h| h.command.as_str())
            .collect();
        assert!(
            cmds.contains(&"/other/tool run"),
            "foreign hook preserved: {cmds:?}"
        );
        assert!(
            cmds.iter()
                .any(|c| c.contains("agent hooks codex session-start")),
            "libra hook added: {cmds:?}"
        );
    }

    // ----- Flat backend (Cursor / Copilot) -----

    #[test]
    fn cursor_flat_install_uses_command_field() {
        let mut file = FlatFile::default();
        let changed = upsert_flat(
            &mut file,
            &CURSOR_SPEC,
            BIN,
            "agent hooks cursor",
            "command",
            false,
            None,
            true,
        );
        assert!(changed);
        let entry = &file.hooks["sessionStart"][0];
        assert_eq!(
            entry.get("command").and_then(Value::as_str),
            Some(format!("{BIN} agent hooks cursor session-start || true").as_str())
        );
        assert!(
            entry.get("type").is_none(),
            "cursor entries carry no type field"
        );
    }

    #[test]
    fn copilot_flat_install_uses_bash_field_with_type_and_comment() {
        let mut file = FlatFile::default();
        upsert_flat(
            &mut file,
            &COPILOT_SPEC,
            BIN,
            "agent hooks copilot",
            "bash",
            true,
            Some("Libra agent capture"),
            true,
        );
        let entry = &file.hooks["postToolUse"][0];
        assert_eq!(entry.get("type").and_then(Value::as_str), Some("command"));
        assert_eq!(
            entry.get("bash").and_then(Value::as_str),
            Some(format!("{BIN} agent hooks copilot tool-use || true").as_str())
        );
        assert_eq!(
            entry.get("comment").and_then(Value::as_str),
            Some("Libra agent capture")
        );
    }

    #[test]
    fn flat_install_is_idempotent_and_preserves_foreign_entries() {
        let mut file = FlatFile::default();
        file.hooks.insert(
            "stop".to_string(),
            vec![Value::Object({
                let mut m = Map::new();
                m.insert(
                    "command".to_string(),
                    Value::String("other thing".to_string()),
                );
                m
            })],
        );
        assert!(upsert_flat(
            &mut file,
            &CURSOR_SPEC,
            BIN,
            "agent hooks cursor",
            "command",
            false,
            None,
            true
        ));
        assert!(
            !upsert_flat(
                &mut file,
                &CURSOR_SPEC,
                BIN,
                "agent hooks cursor",
                "command",
                false,
                None,
                true
            ),
            "second install no-op"
        );
        let stop_cmds: Vec<&str> = file.hooks["stop"]
            .iter()
            .filter_map(|e| e.get("command").and_then(Value::as_str))
            .collect();
        assert!(
            stop_cmds.contains(&"other thing"),
            "foreign entry preserved: {stop_cmds:?}"
        );
    }

    // ----- managed-command detection -----

    #[test]
    fn command_is_managed_matches_slug_marker_through_fail_safe() {
        assert!(command_is_managed(
            "/x/libra agent hooks cursor stop || true",
            "cursor"
        ));
        assert!(!command_is_managed(
            "/x/libra agent hooks codex stop",
            "cursor"
        ));
        assert!(!command_is_managed("/x/other tool", "cursor"));
    }

    // ----- TS plugin -----

    #[test]
    fn ts_plugin_carries_marker_and_routes_events() {
        let ts = render_ts_plugin(BIN, "agent hooks opencode");
        assert!(ts.contains(TS_MARKER));
        assert!(ts.contains("session.created"));
        assert!(ts.contains(&format!("{BIN} agent hooks opencode session-start")));
        assert!(ts.contains(&format!("{BIN} agent hooks opencode session-end")));
        assert!(ts.contains("never break the host agent"));
    }

    // ----- alias parser -----

    #[test]
    fn parse_promoted_hook_event_maps_all_agent_aliases() {
        let env = SessionHookEnvelope {
            hook_event_name: String::new(),
            session_id: "s1".to_string(),
            cwd: "/tmp".to_string(),
            transcript_path: None,
            extra: Map::new(),
        };
        let cases = [
            ("SessionStart", LifecycleEventKind::SessionStart),
            ("session.created", LifecycleEventKind::SessionStart),
            ("beforeSubmitPrompt", LifecycleEventKind::TurnStart),
            ("userPromptSubmitted", LifecycleEventKind::TurnStart),
            ("post-tool-use", LifecycleEventKind::ToolUse),
            ("agentStop", LifecycleEventKind::TurnEnd),
            ("session.idle", LifecycleEventKind::TurnEnd),
            ("sessionEnd", LifecycleEventKind::SessionEnd),
            ("session.deleted", LifecycleEventKind::SessionEnd),
            ("preCompact", LifecycleEventKind::Compaction),
        ];
        for (name, kind) in cases {
            let ev = parse_promoted_hook_event(name, &env).expect("maps");
            assert_eq!(ev.kind, kind, "for {name}");
        }
        assert!(parse_promoted_hook_event("totally-unknown", &env).is_err());
    }

    #[test]
    fn providers_report_authoritative_subcommand_and_slug() {
        for p in [
            &CURSOR_PROVIDER,
            &CODEX_PROVIDER,
            &COPILOT_PROVIDER,
            &FACTORY_PROVIDER,
            &OPENCODE_PROVIDER,
        ] {
            assert!(p.subcommand_is_authoritative());
            assert!(!p.provider_name().is_empty());
            assert!(!p.supported_commands().is_empty());
        }
    }
}
