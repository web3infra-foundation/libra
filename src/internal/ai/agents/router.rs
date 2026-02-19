//! Agent router: auto-selects the appropriate agent based on user input.

use super::parser::AgentDefinition;

/// Routes user input to the most appropriate agent definition.
pub struct AgentRouter {
    agents: Vec<AgentDefinition>,
}

impl AgentRouter {
    /// Create a new router with the given agent definitions.
    pub fn new(agents: Vec<AgentDefinition>) -> Self {
        Self { agents }
    }

    /// Select the best matching agent for the given user input.
    ///
    /// Matching is done by checking if keywords from the agent's description
    /// appear in the user input. Returns the agent with the highest match score,
    /// or None if no agent matches above a minimum threshold.
    pub fn select(&self, input: &str) -> Option<&AgentDefinition> {
        let input_lower = input.to_lowercase();
        let mut best: Option<(&AgentDefinition, usize)> = None;

        for agent in &self.agents {
            let score = Self::match_score(&input_lower, agent);
            if score > 0
                && best.as_ref().is_none_or(|(_, best_score)| score > *best_score)
            {
                best = Some((agent, score));
            }
        }

        best.map(|(agent, _)| agent)
    }

    /// Get all registered agent definitions.
    pub fn agents(&self) -> &[AgentDefinition] {
        &self.agents
    }

    /// Get an agent by name.
    pub fn get(&self, name: &str) -> Option<&AgentDefinition> {
        self.agents.iter().find(|a| a.name == name)
    }

    /// Calculate a match score for an agent against user input.
    fn match_score(input_lower: &str, agent: &AgentDefinition) -> usize {
        let keywords = Self::extract_keywords(&agent.description);
        keywords
            .iter()
            .filter(|kw| input_lower.contains(kw.as_str()))
            .count()
    }

    /// Extract meaningful keywords from a description string.
    fn extract_keywords(description: &str) -> Vec<String> {
        let stop_words = [
            "a", "an", "the", "is", "are", "was", "were", "be", "been", "being",
            "have", "has", "had", "do", "does", "did", "will", "would", "could",
            "should", "may", "might", "shall", "can", "for", "and", "but", "or",
            "nor", "not", "so", "yet", "to", "of", "in", "on", "at", "by", "with",
            "from", "up", "about", "into", "through", "during", "before", "after",
            "above", "below", "between", "use", "that", "this", "it", "its",
        ];

        description
            .to_lowercase()
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| w.len() > 2 && !stop_words.contains(w))
            .map(String::from)
            .collect()
    }
}

/// Load all embedded default agent definitions.
pub fn load_embedded_agents() -> Vec<AgentDefinition> {
    let sources = [
        include_str!("embedded/planner.md"),
        include_str!("embedded/code_reviewer.md"),
        include_str!("embedded/architect.md"),
        include_str!("embedded/build_error_resolver.md"),
    ];

    sources
        .iter()
        .filter_map(|src| super::parser::parse_agent_definition(src))
        .collect()
}

/// Load agent definitions from a directory, with embedded defaults as fallback.
///
/// Checks for agent files in:
/// 1. `{working_dir}/.libra/agents/*.md`
/// 2. `~/.config/libra/agents/*.md`
/// 3. Embedded defaults
pub fn load_agents(working_dir: &std::path::Path) -> Vec<AgentDefinition> {
    let mut agents = Vec::new();
    let mut loaded_names = std::collections::HashSet::new();

    // 1. Project-local agents
    let project_dir = working_dir.join(".libra").join("agents");
    load_agents_from_dir(&project_dir, &mut agents, &mut loaded_names);

    // 2. User-global agents
    if let Some(config_dir) = dirs::config_dir() {
        let user_dir = config_dir.join("libra").join("agents");
        load_agents_from_dir(&user_dir, &mut agents, &mut loaded_names);
    }

    // 3. Embedded defaults (only for names not yet loaded)
    for agent in load_embedded_agents() {
        if loaded_names.insert(agent.name.clone()) {
            agents.push(agent);
        }
    }

    agents
}

fn load_agents_from_dir(
    dir: &std::path::Path,
    agents: &mut Vec<AgentDefinition>,
    loaded_names: &mut std::collections::HashSet<String>,
) {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "md")
                && let Some(agent) = super::parser::load_agent_from_file(&path)
                && loaded_names.insert(agent.name.clone())
            {
                agents.push(agent);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_embedded_agents() {
        let agents = load_embedded_agents();
        assert_eq!(agents.len(), 4);
        let names: Vec<&str> = agents.iter().map(|a| a.name.as_str()).collect();
        assert!(names.contains(&"planner"));
        assert!(names.contains(&"code_reviewer"));
        assert!(names.contains(&"architect"));
        assert!(names.contains(&"build_error_resolver"));
    }

    #[test]
    fn test_router_select_planner() {
        let agents = load_embedded_agents();
        let router = AgentRouter::new(agents);

        let selected = router.select("plan the implementation of the new feature");
        assert!(selected.is_some());
        assert_eq!(selected.unwrap().name, "planner");
    }

    #[test]
    fn test_router_select_reviewer() {
        let agents = load_embedded_agents();
        let router = AgentRouter::new(agents);

        let selected = router.select("review this code for quality and security");
        assert!(selected.is_some());
        assert_eq!(selected.unwrap().name, "code_reviewer");
    }

    #[test]
    fn test_router_select_architect() {
        let agents = load_embedded_agents();
        let router = AgentRouter::new(agents);

        let selected = router.select("design the system architecture");
        assert!(selected.is_some());
        assert_eq!(selected.unwrap().name, "architect");
    }

    #[test]
    fn test_router_select_build_resolver() {
        let agents = load_embedded_agents();
        let router = AgentRouter::new(agents);

        let selected = router.select("fix the build error compilation failure");
        assert!(selected.is_some());
        assert_eq!(selected.unwrap().name, "build_error_resolver");
    }

    #[test]
    fn test_router_no_match() {
        let agents = load_embedded_agents();
        let router = AgentRouter::new(agents);

        let selected = router.select("hello world");
        assert!(selected.is_none());
    }

    #[test]
    fn test_router_get_by_name() {
        let agents = load_embedded_agents();
        let router = AgentRouter::new(agents);

        assert!(router.get("planner").is_some());
        assert!(router.get("nonexistent").is_none());
    }

    #[test]
    fn test_router_tie_breaking_prefers_first() {
        // When two agents have the same score, the first one encountered wins
        let agents = vec![
            AgentDefinition {
                name: "agent_a".to_string(),
                description: "review code quality".to_string(),
                tools: vec![],
                model_preference: "default".to_string(),
                system_prompt: "A".to_string(),
            },
            AgentDefinition {
                name: "agent_b".to_string(),
                description: "review code quality".to_string(),
                tools: vec![],
                model_preference: "default".to_string(),
                system_prompt: "B".to_string(),
            },
        ];
        let router = AgentRouter::new(agents);

        // Both agents have identical descriptions, so same score
        let selected = router.select("review code quality");
        assert!(selected.is_some());
        assert_eq!(selected.unwrap().name, "agent_a");
    }

    #[test]
    fn test_load_agents_with_project_override() {
        let tmp = tempfile::TempDir::new().unwrap();
        let agents_dir = tmp.path().join(".libra").join("agents");
        std::fs::create_dir_all(&agents_dir).unwrap();
        std::fs::write(
            agents_dir.join("planner.md"),
            "---\nname: planner\ndescription: Custom planner\ntools: []\nmodel: fast\n---\nCustom body",
        ).unwrap();

        let agents = load_agents(tmp.path());
        let planner = agents.iter().find(|a| a.name == "planner").unwrap();
        assert_eq!(planner.description, "Custom planner");
        assert_eq!(planner.model_preference, "fast");
    }
}
