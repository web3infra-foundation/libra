//! Agent profile router: auto-selects the appropriate profile based on user input.

use super::parser::AgentProfile;

const MIN_MATCH_SCORE: usize = 2;
const MAX_PROFILE_FILE_BYTES: u64 = 1024 * 1024;

/// Routes user input to the most appropriate agent profile.
pub struct AgentProfileRouter {
    profiles: Vec<AgentProfile>,
}

impl AgentProfileRouter {
    /// Create a new router with the given agent profiles.
    pub fn new(profiles: Vec<AgentProfile>) -> Self {
        Self { profiles }
    }

    /// Select the best matching profile for the given user input.
    ///
    /// Matching is done by checking if keywords from the profile description
    /// appear in the user input. Returns the profile with the highest match score,
    /// or None if no profile matches above a minimum threshold.
    pub fn select(&self, input: &str) -> Option<&AgentProfile> {
        let input_lower = input.to_lowercase();
        let mut best: Option<(&AgentProfile, usize)> = None;

        for profile in &self.profiles {
            let score = Self::match_score(&input_lower, profile);
            // Require at least 2 keyword matches to avoid false positives
            // on short or generic inputs like "test", "build", etc.
            if score >= MIN_MATCH_SCORE
                && best
                    .as_ref()
                    .is_none_or(|(_, best_score)| score > *best_score)
            {
                best = Some((profile, score));
            }
        }

        best.map(|(profile, _)| profile)
    }

    /// Get all registered profiles.
    pub fn profiles(&self) -> &[AgentProfile] {
        &self.profiles
    }

    /// Get a profile by name.
    pub fn get(&self, name: &str) -> Option<&AgentProfile> {
        self.profiles.iter().find(|a| a.name == name)
    }

    /// Calculate a match score for a profile against user input.
    fn match_score(input_lower: &str, profile: &AgentProfile) -> usize {
        let keywords = Self::extract_keywords(&profile.description);
        keywords
            .iter()
            .filter(|kw| input_lower.contains(kw.as_str()))
            .count()
    }

    /// Extract meaningful keywords from a description string.
    fn extract_keywords(description: &str) -> Vec<String> {
        let stop_words = [
            "a", "an", "the", "is", "are", "was", "were", "be", "been", "being", "have", "has",
            "had", "do", "does", "did", "will", "would", "could", "should", "may", "might",
            "shall", "can", "for", "and", "but", "or", "nor", "not", "so", "yet", "to", "of", "in",
            "on", "at", "by", "with", "from", "up", "about", "into", "through", "during", "before",
            "after", "above", "below", "between", "use", "that", "this", "it", "its",
        ];

        description
            .to_lowercase()
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| w.len() > 2 && !stop_words.contains(w))
            .map(String::from)
            .collect()
    }
}

/// Load all embedded default agent profiles.
pub fn load_embedded_profiles() -> Vec<AgentProfile> {
    let sources = [
        include_str!("embedded/planner.md"),
        include_str!("embedded/code_reviewer.md"),
        include_str!("embedded/architect.md"),
        include_str!("embedded/build_error_resolver.md"),
    ];

    sources
        .iter()
        .filter_map(|src| super::parser::parse_agent_profile(src))
        .collect()
}

/// Load agent profiles from a directory, with embedded profiles as fallback.
///
/// Checks for agent files in:
/// 1. `{working_dir}/.libra/agents/*.md`
/// 2. `~/.config/libra/agents/*.md`
/// 3. Embedded defaults
pub fn load_profiles(working_dir: &std::path::Path) -> Vec<AgentProfile> {
    let mut profiles = Vec::new();
    let mut loaded_names = std::collections::HashSet::new();

    // 1. Project-local profiles
    let project_dir = working_dir.join(".libra").join("agents");
    load_profiles_from_dir(&project_dir, &mut profiles, &mut loaded_names);

    // 2. User-global profiles
    if let Some(config_dir) = dirs::config_dir() {
        let user_dir = config_dir.join("libra").join("agents");
        load_profiles_from_dir(&user_dir, &mut profiles, &mut loaded_names);
    }

    // 3. Embedded defaults (only for names not yet loaded)
    for profile in load_embedded_profiles() {
        if loaded_names.insert(profile.name.clone()) {
            profiles.push(profile);
        }
    }

    profiles
}

fn load_profiles_from_dir(
    dir: &std::path::Path,
    profiles: &mut Vec<AgentProfile>,
    loaded_names: &mut std::collections::HashSet<String>,
) {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|ext| ext != "md") {
                continue;
            }

            let metadata = match path.metadata() {
                Ok(meta) => meta,
                Err(error) => {
                    tracing::warn!(path = %path.display(), error = %error, "failed to read agent file metadata");
                    continue;
                }
            };

            if metadata.len() > MAX_PROFILE_FILE_BYTES {
                tracing::warn!(
                    path = %path.display(),
                    size = metadata.len(),
                    max_bytes = MAX_PROFILE_FILE_BYTES,
                    "skipped oversized agent profile",
                );
                continue;
            }

            if let Some(profile) = super::parser::load_agent_profile_from_file(&path)
                && loaded_names.insert(profile.name.clone())
            {
                profiles.push(profile);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_profiles_skips_oversized_files() {
        let tmp = tempfile::tempdir().unwrap();
        let agents_dir = tmp.path().join(".libra").join("agents");
        std::fs::create_dir_all(&agents_dir).unwrap();

        let valid_profile = agents_dir.join("valid.md");
        std::fs::write(
            &valid_profile,
            "---\nname: valid\ndescription: Valid planner\ntools: []\nmodel: default\n---\nbody",
        )
        .unwrap();

        let mut oversized = String::from(
            "---\nname: oversized\ndescription: Oversized profile\ntools: []\nmodel: default\n---\n",
        );
        oversized.push_str(&"a".repeat((MAX_PROFILE_FILE_BYTES + 1) as usize));
        std::fs::write(agents_dir.join("oversized.md"), oversized).unwrap();

        let profiles = load_profiles(tmp.path());
        let names: Vec<_> = profiles.iter().map(|p| p.name.as_str()).collect();
        assert!(names.contains(&"valid"));
        assert!(!names.contains(&"oversized"));
    }

    #[test]
    fn test_load_embedded_profiles() {
        let profiles = load_embedded_profiles();
        assert_eq!(profiles.len(), 4);
        let names: Vec<&str> = profiles.iter().map(|a| a.name.as_str()).collect();
        assert!(names.contains(&"planner"));
        assert!(names.contains(&"code_reviewer"));
        assert!(names.contains(&"architect"));
        assert!(names.contains(&"build_error_resolver"));
    }

    #[test]
    fn test_router_select_planner() {
        let profiles = load_embedded_profiles();
        let router = AgentProfileRouter::new(profiles);

        let selected =
            router.select("plan the implementation and identify dependencies for the new feature");
        assert!(selected.is_some());
        assert_eq!(selected.unwrap().name, "planner");
    }

    #[test]
    fn test_router_select_reviewer() {
        let profiles = load_embedded_profiles();
        let router = AgentProfileRouter::new(profiles);

        let selected = router.select("review this code for quality and security");
        assert!(selected.is_some());
        assert_eq!(selected.unwrap().name, "code_reviewer");
    }

    #[test]
    fn test_router_select_architect() {
        let profiles = load_embedded_profiles();
        let router = AgentProfileRouter::new(profiles);

        let selected = router.select("design the system architecture");
        assert!(selected.is_some());
        assert_eq!(selected.unwrap().name, "architect");
    }

    #[test]
    fn test_router_select_build_resolver() {
        let profiles = load_embedded_profiles();
        let router = AgentProfileRouter::new(profiles);

        let selected = router.select("fix the build error compilation failure");
        assert!(selected.is_some());
        assert_eq!(selected.unwrap().name, "build_error_resolver");
    }

    #[test]
    fn test_router_no_match() {
        let profiles = load_embedded_profiles();
        let router = AgentProfileRouter::new(profiles);

        let selected = router.select("hello world");
        assert!(selected.is_none());
    }

    #[test]
    fn test_router_get_by_name() {
        let profiles = load_embedded_profiles();
        let router = AgentProfileRouter::new(profiles);

        assert!(router.get("planner").is_some());
        assert!(router.get("nonexistent").is_none());
    }

    #[test]
    fn test_router_tie_breaking_prefers_first() {
        // When two profiles have the same score, the first one encountered wins
        let profiles = vec![
            AgentProfile {
                name: "agent_a".to_string(),
                description: "review code quality".to_string(),
                tools: vec![],
                model_preference: "default".to_string(),
                system_prompt: "A".to_string(),
            },
            AgentProfile {
                name: "agent_b".to_string(),
                description: "review code quality".to_string(),
                tools: vec![],
                model_preference: "default".to_string(),
                system_prompt: "B".to_string(),
            },
        ];
        let router = AgentProfileRouter::new(profiles);

        // Both profiles have identical descriptions, so same score
        let selected = router.select("review code quality");
        assert!(selected.is_some());
        assert_eq!(selected.unwrap().name, "agent_a");
    }

    #[test]
    fn test_load_profiles_with_project_override() {
        let tmp = tempfile::TempDir::new().unwrap();
        let agents_dir = tmp.path().join(".libra").join("agents");
        std::fs::create_dir_all(&agents_dir).unwrap();
        std::fs::write(
            agents_dir.join("planner.md"),
            "---\nname: planner\ndescription: Custom planner\ntools: []\nmodel: fast\n---\nCustom body",
        )
        .unwrap();

        let profiles = load_profiles(tmp.path());
        let planner = profiles.iter().find(|a| a.name == "planner").unwrap();
        assert_eq!(planner.description, "Custom planner");
        assert_eq!(planner.model_preference, "fast");
    }
}
