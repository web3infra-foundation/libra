//! Statically registered lifecycle hook providers.
//!
//! Each provider lives in its own submodule and exposes a singleton
//! `&'static dyn HookProvider`. Lookup goes through [`find_provider`]; the
//! up-front `match` is intentional so that adding a new provider is a single
//! visible change (rather than a runtime registry that's harder to audit).

pub mod claude;
pub mod gemini;

use super::provider::HookProvider;

/// Provider names that ship with Libra. Used by CLI completion / help text.
const SUPPORTED_PROVIDER_NAMES: &[&str] = &["claude", "gemini"];

/// Singleton accessor for the Claude hook provider.
pub fn claude_provider() -> &'static dyn HookProvider {
    &claude::CLAUDE_PROVIDER
}

/// Singleton accessor for the Gemini hook provider.
pub fn gemini_provider() -> &'static dyn HookProvider {
    &gemini::GEMINI_PROVIDER
}

/// List of provider name strings recognised by [`find_provider`].
pub fn supported_provider_names() -> &'static [&'static str] {
    SUPPORTED_PROVIDER_NAMES
}

/// Resolve a provider by name. Returns `None` for unknown providers so callers
/// can surface a friendly error rather than panicking.
pub fn find_provider(provider_name: &str) -> Option<&'static dyn HookProvider> {
    match provider_name {
        "claude" => Some(claude_provider()),
        "gemini" => Some(gemini_provider()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Scenario: lookup succeeds for the two registered providers and rejects
    // anything else.
    #[test]
    fn registry_finds_known_providers() {
        assert_eq!(
            find_provider("claude").map(HookProvider::provider_name),
            Some("claude")
        );
        assert_eq!(
            find_provider("gemini").map(HookProvider::provider_name),
            Some("gemini")
        );
        assert!(find_provider("unknown").is_none());
        assert_eq!(supported_provider_names(), &["claude", "gemini"]);
    }
}
