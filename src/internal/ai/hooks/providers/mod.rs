//! Statically registered lifecycle hook providers.

pub mod claude;
pub mod gemini;

use super::provider::HookProvider;

const SUPPORTED_PROVIDER_NAMES: &[&str] = &["claude", "gemini"];

pub fn claude_provider() -> &'static dyn HookProvider {
    &claude::CLAUDE_PROVIDER
}

pub fn gemini_provider() -> &'static dyn HookProvider {
    &gemini::GEMINI_PROVIDER
}

pub fn supported_provider_names() -> &'static [&'static str] {
    SUPPORTED_PROVIDER_NAMES
}

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
