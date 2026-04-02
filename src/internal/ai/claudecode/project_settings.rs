use serde_json::{Map, Value, json};
use thiserror::Error;

use super::*;

const CLAUDE_SETTINGS_DIR: &str = ".claude";
const CLAUDE_SHARED_SETTINGS_FILE: &str = "settings.json";
const CLAUDE_LOCAL_SETTINGS_FILE: &str = "settings.local.json";
const CLAUDE_PLANS_DIR: &str = "plans";
const LIBRA_DENY_RULES: [&str; 2] = ["Read(/.libra/**)", "Edit(/.libra/**)"];
const GATEWAY_API_KEY_WARNING: &str = "warning: ANTHROPIC_BASE_URL is set but ANTHROPIC_AUTH_TOKEN is not. Some third-party gateways accept raw /v1/messages with x-api-key but reject Claude Code / Python SDK traffic unless Authorization bearer auth is configured.";

#[derive(Debug, Error)]
pub(crate) enum ClaudecodeProjectSettingsError {
    #[error(
        "missing Anthropic credentials; configure .claude/settings.local.json apiKeyHelper, env.ANTHROPIC_AUTH_TOKEN, env.ANTHROPIC_API_KEY, or process ANTHROPIC_AUTH_TOKEN/ANTHROPIC_API_KEY before running `libra code --provider claudecode`"
    )]
    MissingCredentials,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClaudeCredentialSource {
    LocalApiKeyHelper,
    LocalAuthToken,
    LocalApiKey,
    ProcessAuthToken,
    ProcessApiKey,
}

impl ClaudeCredentialSource {
    fn label(self) -> &'static str {
        match self {
            Self::LocalApiKeyHelper => "local apiKeyHelper",
            Self::LocalAuthToken => "local env.ANTHROPIC_AUTH_TOKEN",
            Self::LocalApiKey => "local env.ANTHROPIC_API_KEY",
            Self::ProcessAuthToken => "process env.ANTHROPIC_AUTH_TOKEN",
            Self::ProcessApiKey => "process env.ANTHROPIC_API_KEY",
        }
    }

    pub(crate) fn request_value(self) -> &'static str {
        match self {
            Self::LocalApiKeyHelper => "local_api_key_helper",
            Self::LocalAuthToken => "local_env_auth_token",
            Self::LocalApiKey => "local_env_api_key",
            Self::ProcessAuthToken => "process_env_auth_token",
            Self::ProcessApiKey => "process_env_api_key",
        }
    }

    fn uses_api_key(self) -> bool {
        matches!(self, Self::LocalApiKey | Self::ProcessApiKey)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ClaudeProjectBootstrapStatus {
    claude_dir_created: bool,
    shared_settings_created: bool,
    shared_settings_updated: bool,
    local_settings_created: bool,
    plans_dir_created: bool,
}

impl ClaudeProjectBootstrapStatus {
    fn status_line(&self) -> String {
        let mut actions = Vec::new();
        if self.claude_dir_created {
            actions.push("created .claude/");
        }
        if self.shared_settings_created {
            actions.push("created .claude/settings.json");
        } else if self.shared_settings_updated {
            actions.push("updated .claude/settings.json");
        }
        if self.local_settings_created {
            actions.push("created .claude/settings.local.json");
        }
        if self.plans_dir_created {
            actions.push("created .claude/plans/");
        }

        if actions.is_empty() {
            "detected existing .claude config".to_string()
        } else {
            format!("bootstrapped {}", actions.join(", "))
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ClaudecodeProjectResolvedSettings {
    pub(crate) plans_directory: Option<String>,
    pub(crate) permissions_deny: Vec<String>,
    pub(crate) credential_source: ClaudeCredentialSource,
    pub(crate) base_url: Option<String>,
    local_api_key_helper: Option<String>,
    local_auth_token: Option<String>,
    local_api_key: Option<String>,
    local_base_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ClaudecodeProjectBootstrap {
    pub(crate) provider_env_overrides: BTreeMap<String, String>,
    pub(crate) provider_env_unset: Vec<String>,
    pub(crate) credential_source: Option<ClaudeCredentialSource>,
    pub(crate) startup_note: String,
}

pub(crate) fn is_auth_error(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<ClaudecodeProjectSettingsError>()
            .is_some_and(|err| matches!(err, ClaudecodeProjectSettingsError::MissingCredentials))
    })
}

pub(crate) async fn prepare_claudecode_project_bootstrap(
    storage_path: &Path,
) -> Result<ClaudecodeProjectBootstrap> {
    let project_root = resolve_project_root(storage_path)?;
    let bootstrap = bootstrap_claude_project_settings(&project_root).await?;
    let process_env = std::env::vars().collect::<BTreeMap<_, _>>();
    let resolved = resolve_claude_project_settings(&project_root, &process_env).await?;
    let (provider_env_overrides, provider_env_unset) = build_provider_env_instructions(&resolved)?;

    let mut startup_lines = vec![format!(
        "Claude project settings: {}; credentials: {}",
        bootstrap.status_line(),
        resolved.credential_source.label()
    )];
    if resolved
        .base_url
        .as_ref()
        .filter(|_| resolved.credential_source.uses_api_key())
        .is_some()
    {
        startup_lines.push(GATEWAY_API_KEY_WARNING.to_string());
    }

    Ok(ClaudecodeProjectBootstrap {
        provider_env_overrides,
        provider_env_unset,
        credential_source: Some(resolved.credential_source),
        startup_note: startup_lines.join("\n"),
    })
}

async fn bootstrap_claude_project_settings(cwd: &Path) -> Result<ClaudeProjectBootstrapStatus> {
    let mut status = ClaudeProjectBootstrapStatus::default();
    let claude_dir = cwd.join(CLAUDE_SETTINGS_DIR);
    let shared_settings_path = claude_dir.join(CLAUDE_SHARED_SETTINGS_FILE);
    let local_settings_path = claude_dir.join(CLAUDE_LOCAL_SETTINGS_FILE);
    let plans_dir = claude_dir.join(CLAUDE_PLANS_DIR);

    if !claude_dir.exists() {
        fs::create_dir_all(&claude_dir).await.with_context(|| {
            format!(
                "failed to create Claude settings directory '{}'",
                claude_dir.display()
            )
        })?;
        status.claude_dir_created = true;
    }

    let mut shared_settings =
        load_settings_object(&shared_settings_path, "Claude shared settings").await?;
    let shared_exists = shared_settings_path.exists();
    let shared_changed = ensure_libra_deny_rules(&mut shared_settings)?;
    if !shared_exists || shared_changed {
        write_pretty_json_file(&shared_settings_path, &shared_settings)
            .await
            .with_context(|| {
                format!(
                    "failed to write Claude shared settings '{}'",
                    shared_settings_path.display()
                )
            })?;
        status.shared_settings_created = !shared_exists;
        status.shared_settings_updated = shared_exists && shared_changed;
    }

    if !local_settings_path.exists() {
        write_pretty_json_file(
            &local_settings_path,
            &json!({ "plansDirectory": ".claude/plans" }),
        )
        .await
        .with_context(|| {
            format!(
                "failed to write Claude local settings '{}'",
                local_settings_path.display()
            )
        })?;
        status.local_settings_created = true;
    }

    if !plans_dir.exists() {
        fs::create_dir_all(&plans_dir).await.with_context(|| {
            format!(
                "failed to create Claude plans directory '{}'",
                plans_dir.display()
            )
        })?;
        status.plans_dir_created = true;
    }

    Ok(status)
}

pub(crate) async fn resolve_claude_project_settings(
    cwd: &Path,
    process_env: &BTreeMap<String, String>,
) -> Result<ClaudecodeProjectResolvedSettings> {
    let claude_dir = cwd.join(CLAUDE_SETTINGS_DIR);
    let shared_settings = load_settings_object(
        &claude_dir.join(CLAUDE_SHARED_SETTINGS_FILE),
        "Claude shared settings",
    )
    .await?;
    let local_settings = load_settings_object(
        &claude_dir.join(CLAUDE_LOCAL_SETTINGS_FILE),
        "Claude local settings",
    )
    .await?;

    let local_plans_directory = read_optional_string(&local_settings, "plansDirectory")
        .context("failed to read Claude local settings plansDirectory")?;
    let shared_plans_directory = read_optional_string(&shared_settings, "plansDirectory")
        .context("failed to read Claude shared settings plansDirectory")?;
    let plans_directory = local_plans_directory.or(shared_plans_directory);

    let local_api_key_helper = read_optional_string(&local_settings, "apiKeyHelper")
        .context("failed to read Claude local settings apiKeyHelper")?;
    let local_auth_token = read_env_value(&local_settings, "ANTHROPIC_AUTH_TOKEN")
        .context("failed to read Claude local settings env.ANTHROPIC_AUTH_TOKEN")?;
    let local_api_key = read_env_value(&local_settings, "ANTHROPIC_API_KEY")
        .context("failed to read Claude local settings env.ANTHROPIC_API_KEY")?;
    let local_base_url = read_env_value(&local_settings, "ANTHROPIC_BASE_URL")
        .context("failed to read Claude local settings env.ANTHROPIC_BASE_URL")?;

    let permissions_deny = merged_permissions_deny(&shared_settings, &local_settings)
        .context("failed to read Claude permissions.deny rules")?;

    let credential_source = if local_api_key_helper.is_some() {
        Some(ClaudeCredentialSource::LocalApiKeyHelper)
    } else if local_auth_token.is_some() {
        Some(ClaudeCredentialSource::LocalAuthToken)
    } else if local_api_key.is_some() {
        Some(ClaudeCredentialSource::LocalApiKey)
    } else if non_empty_env(process_env, "ANTHROPIC_AUTH_TOKEN").is_some() {
        Some(ClaudeCredentialSource::ProcessAuthToken)
    } else if non_empty_env(process_env, "ANTHROPIC_API_KEY").is_some() {
        Some(ClaudeCredentialSource::ProcessApiKey)
    } else {
        None
    }
    .ok_or(ClaudecodeProjectSettingsError::MissingCredentials)?;

    let base_url = local_base_url
        .clone()
        .or_else(|| non_empty_env(process_env, "ANTHROPIC_BASE_URL"));

    Ok(ClaudecodeProjectResolvedSettings {
        plans_directory,
        permissions_deny,
        credential_source,
        base_url,
        local_api_key_helper,
        local_auth_token,
        local_api_key,
        local_base_url,
    })
}

fn resolve_project_root(storage_path: &Path) -> Result<PathBuf> {
    if storage_path.file_name().and_then(|name| name.to_str()) == Some(crate::utils::util::ROOT_DIR)
    {
        return storage_path.parent().map(Path::to_path_buf).ok_or_else(|| {
            anyhow!(
                "failed to resolve repository root from Libra storage path '{}'",
                storage_path.display()
            )
        });
    }

    Ok(storage_path.to_path_buf())
}

fn build_provider_env_instructions(
    resolved: &ClaudecodeProjectResolvedSettings,
) -> Result<(BTreeMap<String, String>, Vec<String>)> {
    let mut overrides = BTreeMap::new();
    let mut unset = BTreeSet::new();

    if let Some(base_url) = resolved.local_base_url.as_ref() {
        overrides.insert("ANTHROPIC_BASE_URL".to_string(), base_url.clone());
    }

    match resolved.credential_source {
        ClaudeCredentialSource::LocalApiKeyHelper => {
            unset.insert("ANTHROPIC_AUTH_TOKEN".to_string());
            unset.insert("ANTHROPIC_API_KEY".to_string());
        }
        ClaudeCredentialSource::LocalAuthToken => {
            let token = resolved.local_auth_token.clone().ok_or_else(|| {
                anyhow!("local credential source selected without local auth token")
            })?;
            overrides.insert("ANTHROPIC_AUTH_TOKEN".to_string(), token);
            unset.insert("ANTHROPIC_API_KEY".to_string());
        }
        ClaudeCredentialSource::LocalApiKey => {
            let api_key = resolved
                .local_api_key
                .clone()
                .ok_or_else(|| anyhow!("local credential source selected without local API key"))?;
            overrides.insert("ANTHROPIC_API_KEY".to_string(), api_key);
            unset.insert("ANTHROPIC_AUTH_TOKEN".to_string());
        }
        ClaudeCredentialSource::ProcessAuthToken => {
            unset.insert("ANTHROPIC_API_KEY".to_string());
        }
        ClaudeCredentialSource::ProcessApiKey => {}
    }

    if resolved.local_api_key_helper.is_some() {
        unset.insert("ANTHROPIC_AUTH_TOKEN".to_string());
        unset.insert("ANTHROPIC_API_KEY".to_string());
    }

    Ok((overrides, unset.into_iter().collect()))
}

async fn load_settings_object(path: &Path, label: &str) -> Result<Value> {
    if !path.exists() {
        return Ok(Value::Object(Map::new()));
    }
    let value = read_json_artifact::<Value>(path, label).await?;
    if value.is_null() {
        return Ok(Value::Object(Map::new()));
    }
    if !value.is_object() {
        bail!("{label} '{}' must contain a JSON object", path.display());
    }
    Ok(value)
}

fn ensure_libra_deny_rules(settings: &mut Value) -> Result<bool> {
    let root = ensure_object_mut(settings, "Claude shared settings root")?;
    let permissions =
        ensure_child_object_mut(root, "permissions", "Claude shared settings permissions")?;
    let deny = ensure_string_array_mut(
        permissions,
        "deny",
        "Claude shared settings permissions.deny",
    )?;

    let existing = deny
        .iter()
        .map(|value| {
            value
                .as_str()
                .ok_or_else(|| {
                    anyhow!("Claude shared settings permissions.deny must contain only strings")
                })
                .map(str::to_string)
        })
        .collect::<Result<BTreeSet<_>>>()?;

    let mut changed = false;
    for rule in LIBRA_DENY_RULES {
        if !existing.contains(rule) {
            deny.push(Value::String(rule.to_string()));
            changed = true;
        }
    }
    Ok(changed)
}

fn merged_permissions_deny(shared_settings: &Value, local_settings: &Value) -> Result<Vec<String>> {
    let mut merged = Vec::new();
    for value in [shared_settings, local_settings] {
        for rule in read_permissions_deny(value)? {
            if !merged.contains(&rule) {
                merged.push(rule);
            }
        }
    }
    Ok(merged)
}

fn read_permissions_deny(settings: &Value) -> Result<Vec<String>> {
    let Some(root) = settings.as_object() else {
        return Ok(Vec::new());
    };
    let Some(permissions) = root.get("permissions") else {
        return Ok(Vec::new());
    };
    let Some(permissions) = permissions.as_object() else {
        bail!("Claude settings permissions must be a JSON object");
    };
    let Some(deny) = permissions.get("deny") else {
        return Ok(Vec::new());
    };
    let Some(deny) = deny.as_array() else {
        bail!("Claude settings permissions.deny must be a JSON array");
    };
    deny.iter()
        .map(|value| {
            value
                .as_str()
                .map(|text| text.trim().to_string())
                .filter(|text| !text.is_empty())
                .ok_or_else(|| {
                    anyhow!("Claude settings permissions.deny must contain only non-empty strings")
                })
        })
        .collect()
}

fn read_env_value(settings: &Value, key: &str) -> Result<Option<String>> {
    let Some(root) = settings.as_object() else {
        return Ok(None);
    };
    let Some(env) = root.get("env") else {
        return Ok(None);
    };
    let Some(env) = env.as_object() else {
        bail!("Claude settings env must be a JSON object");
    };
    match env.get(key) {
        Some(value) => read_trimmed_string_value(value, &format!("Claude settings env.{key}")),
        None => Ok(None),
    }
}

fn read_optional_string(settings: &Value, key: &str) -> Result<Option<String>> {
    let Some(root) = settings.as_object() else {
        return Ok(None);
    };
    match root.get(key) {
        Some(value) => read_trimmed_string_value(value, &format!("Claude settings {key}")),
        None => Ok(None),
    }
}

fn read_trimmed_string_value(value: &Value, label: &str) -> Result<Option<String>> {
    let Some(text) = value.as_str() else {
        bail!("{label} must be a string");
    };
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    Ok(Some(trimmed.to_string()))
}

fn non_empty_env(process_env: &BTreeMap<String, String>, key: &str) -> Option<String> {
    process_env
        .get(key)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn ensure_object_mut<'a>(value: &'a mut Value, label: &str) -> Result<&'a mut Map<String, Value>> {
    value
        .as_object_mut()
        .ok_or_else(|| anyhow!("{label} must be a JSON object"))
}

fn ensure_child_object_mut<'a>(
    object: &'a mut Map<String, Value>,
    key: &str,
    label: &str,
) -> Result<&'a mut Map<String, Value>> {
    let entry = object
        .entry(key.to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    entry
        .as_object_mut()
        .ok_or_else(|| anyhow!("{label} must be a JSON object"))
}

fn ensure_string_array_mut<'a>(
    object: &'a mut Map<String, Value>,
    key: &str,
    label: &str,
) -> Result<&'a mut Vec<Value>> {
    let entry = object
        .entry(key.to_string())
        .or_insert_with(|| Value::Array(Vec::new()));
    entry
        .as_array_mut()
        .ok_or_else(|| anyhow!("{label} must be a JSON array"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn bootstrap_creates_default_claude_project_files() {
        let repo = tempfile::tempdir().expect("tempdir");

        let status = bootstrap_claude_project_settings(repo.path())
            .await
            .expect("bootstrap should succeed");

        assert!(status.claude_dir_created);
        assert!(status.shared_settings_created);
        assert!(status.local_settings_created);
        assert!(status.plans_dir_created);

        let shared = load_settings_object(
            &repo.path().join(".claude").join("settings.json"),
            "shared settings",
        )
        .await
        .expect("shared settings");
        assert_eq!(
            read_permissions_deny(&shared).expect("permissions.deny"),
            LIBRA_DENY_RULES
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        );

        let local = load_settings_object(
            &repo.path().join(".claude").join("settings.local.json"),
            "local settings",
        )
        .await
        .expect("local settings");
        assert_eq!(
            read_optional_string(&local, "plansDirectory").expect("plansDirectory"),
            Some(".claude/plans".to_string())
        );
    }

    #[tokio::test]
    async fn bootstrap_preserves_hooks_and_only_appends_libra_deny_rules() {
        let repo = tempfile::tempdir().expect("tempdir");
        let claude_dir = repo.path().join(".claude");
        fs::create_dir_all(&claude_dir)
            .await
            .expect("create .claude");
        write_pretty_json_file(
            &claude_dir.join("settings.json"),
            &json!({
                "hooks": {
                    "Stop": [
                        {
                            "hooks": [{"type": "command", "command": "echo keep"}]
                        }
                    ]
                },
                "permissions": {
                    "deny": ["Read(/tmp/**)"]
                }
            }),
        )
        .await
        .expect("seed settings");

        bootstrap_claude_project_settings(repo.path())
            .await
            .expect("bootstrap should succeed");

        let shared = load_settings_object(&claude_dir.join("settings.json"), "shared settings")
            .await
            .expect("shared settings");
        assert_eq!(
            shared["hooks"]["Stop"][0]["hooks"][0]["command"],
            json!("echo keep")
        );
        assert_eq!(
            read_permissions_deny(&shared).expect("permissions.deny"),
            vec![
                "Read(/tmp/**)".to_string(),
                "Read(/.libra/**)".to_string(),
                "Edit(/.libra/**)".to_string(),
            ]
        );
    }

    #[tokio::test]
    async fn bootstrap_does_not_overwrite_existing_local_settings() {
        let repo = tempfile::tempdir().expect("tempdir");
        let claude_dir = repo.path().join(".claude");
        fs::create_dir_all(&claude_dir)
            .await
            .expect("create .claude");
        write_pretty_json_file(
            &claude_dir.join("settings.local.json"),
            &json!({
                "plansDirectory": ".custom/plans",
                "env": {
                    "ANTHROPIC_AUTH_TOKEN": "local-token"
                }
            }),
        )
        .await
        .expect("seed local settings");

        let status = bootstrap_claude_project_settings(repo.path())
            .await
            .expect("bootstrap should succeed");
        assert!(!status.local_settings_created);

        let local = load_settings_object(&claude_dir.join("settings.local.json"), "local settings")
            .await
            .expect("local settings");
        assert_eq!(
            read_optional_string(&local, "plansDirectory").expect("plansDirectory"),
            Some(".custom/plans".to_string())
        );
        assert_eq!(
            read_env_value(&local, "ANTHROPIC_AUTH_TOKEN").expect("local auth token"),
            Some("local-token".to_string())
        );
    }

    #[tokio::test]
    async fn resolve_credentials_prefers_local_helper_then_local_env_then_process_env() {
        let repo = tempfile::tempdir().expect("tempdir");
        let claude_dir = repo.path().join(".claude");
        fs::create_dir_all(&claude_dir)
            .await
            .expect("create .claude");

        write_pretty_json_file(
            &claude_dir.join("settings.local.json"),
            &json!({
                "apiKeyHelper": "security find-generic-password -w",
                "env": {
                    "ANTHROPIC_AUTH_TOKEN": "local-token",
                    "ANTHROPIC_API_KEY": "local-api-key"
                }
            }),
        )
        .await
        .expect("seed local settings");

        let mut process_env = BTreeMap::new();
        process_env.insert(
            "ANTHROPIC_AUTH_TOKEN".to_string(),
            "process-token".to_string(),
        );
        process_env.insert(
            "ANTHROPIC_API_KEY".to_string(),
            "process-api-key".to_string(),
        );

        let resolved = resolve_claude_project_settings(repo.path(), &process_env)
            .await
            .expect("resolve settings");
        assert_eq!(
            resolved.credential_source,
            ClaudeCredentialSource::LocalApiKeyHelper
        );

        write_pretty_json_file(
            &claude_dir.join("settings.local.json"),
            &json!({
                "env": {
                    "ANTHROPIC_AUTH_TOKEN": "local-token",
                    "ANTHROPIC_API_KEY": "local-api-key"
                }
            }),
        )
        .await
        .expect("seed local settings without helper");
        let resolved = resolve_claude_project_settings(repo.path(), &process_env)
            .await
            .expect("resolve settings");
        assert_eq!(
            resolved.credential_source,
            ClaudeCredentialSource::LocalAuthToken
        );

        write_pretty_json_file(
            &claude_dir.join("settings.local.json"),
            &json!({
                "env": {
                    "ANTHROPIC_API_KEY": "local-api-key"
                }
            }),
        )
        .await
        .expect("seed local api key");
        let resolved = resolve_claude_project_settings(repo.path(), &process_env)
            .await
            .expect("resolve settings");
        assert_eq!(
            resolved.credential_source,
            ClaudeCredentialSource::LocalApiKey
        );

        write_pretty_json_file(&claude_dir.join("settings.local.json"), &json!({}))
            .await
            .expect("seed empty local settings");
        let resolved = resolve_claude_project_settings(repo.path(), &process_env)
            .await
            .expect("resolve settings");
        assert_eq!(
            resolved.credential_source,
            ClaudeCredentialSource::ProcessAuthToken
        );

        process_env.remove("ANTHROPIC_AUTH_TOKEN");
        let resolved = resolve_claude_project_settings(repo.path(), &process_env)
            .await
            .expect("resolve settings");
        assert_eq!(
            resolved.credential_source,
            ClaudeCredentialSource::ProcessApiKey
        );
    }

    #[tokio::test]
    async fn resolve_rejects_missing_credentials_even_when_base_url_exists() {
        let repo = tempfile::tempdir().expect("tempdir");
        let claude_dir = repo.path().join(".claude");
        fs::create_dir_all(&claude_dir)
            .await
            .expect("create .claude");
        write_pretty_json_file(
            &claude_dir.join("settings.local.json"),
            &json!({
                "env": {
                    "ANTHROPIC_BASE_URL": "https://gateway.example.com"
                }
            }),
        )
        .await
        .expect("seed local settings");

        let err = resolve_claude_project_settings(repo.path(), &BTreeMap::new())
            .await
            .expect_err("missing credentials should fail");
        assert!(is_auth_error(&err));
    }

    #[tokio::test]
    async fn resolve_allows_missing_base_url_when_auth_exists() {
        let repo = tempfile::tempdir().expect("tempdir");
        let claude_dir = repo.path().join(".claude");
        fs::create_dir_all(&claude_dir)
            .await
            .expect("create .claude");
        write_pretty_json_file(
            &claude_dir.join("settings.local.json"),
            &json!({
                "env": {
                    "ANTHROPIC_AUTH_TOKEN": "local-token"
                }
            }),
        )
        .await
        .expect("seed local settings");

        let resolved = resolve_claude_project_settings(repo.path(), &BTreeMap::new())
            .await
            .expect("auth-only config should succeed");
        assert_eq!(
            resolved.credential_source,
            ClaudeCredentialSource::LocalAuthToken
        );
        assert_eq!(resolved.base_url, None);
    }
}
