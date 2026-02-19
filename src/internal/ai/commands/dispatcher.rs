//! Slash command dispatcher: routes `/command args` to the right handler.

use super::parser::CommandDefinition;

/// Dispatches slash commands to their definitions.
pub struct CommandDispatcher {
    commands: Vec<CommandDefinition>,
}

/// Result of dispatching a slash command.
#[derive(Debug, Clone)]
pub struct DispatchResult {
    /// The expanded prompt to send to the model.
    pub prompt: String,
    /// Optional agent name to use for this command.
    pub agent: Option<String>,
}

impl CommandDispatcher {
    /// Create a new dispatcher with the given command definitions.
    pub fn new(commands: Vec<CommandDefinition>) -> Self {
        Self { commands }
    }

    /// Try to dispatch user input as a slash command.
    ///
    /// Returns `Some(DispatchResult)` if the input starts with a known `/command`,
    /// or `None` if it's not a slash command.
    pub fn dispatch(&self, input: &str) -> Option<DispatchResult> {
        let input = input.trim();
        if !input.starts_with('/') {
            return None;
        }

        let without_slash = &input[1..];
        let (cmd_name, arguments) = match without_slash.split_once(char::is_whitespace) {
            Some((name, args)) => (name, args.trim()),
            None => (without_slash, ""),
        };

        let command = self.commands.iter().find(|c| c.name == cmd_name)?;

        Some(DispatchResult {
            prompt: command.expand(arguments),
            agent: command.agent.clone(),
        })
    }

    /// Get all registered command definitions.
    pub fn commands(&self) -> &[CommandDefinition] {
        &self.commands
    }

    /// Get a command by name.
    pub fn get(&self, name: &str) -> Option<&CommandDefinition> {
        self.commands.iter().find(|c| c.name == name)
    }
}

/// Load all embedded default command definitions.
pub fn load_embedded_commands() -> Vec<CommandDefinition> {
    let sources = [
        include_str!("embedded/plan.md"),
        include_str!("embedded/code_review.md"),
        include_str!("embedded/verify.md"),
        include_str!("embedded/tdd.md"),
    ];

    sources
        .iter()
        .filter_map(|src| super::parser::parse_command_definition(src))
        .collect()
}

/// Load command definitions from the three-tier hierarchy.
///
/// 1. `{working_dir}/.libra/commands/*.md` (project-local)
/// 2. `~/.config/libra/commands/*.md` (user-global)
/// 3. Embedded defaults
pub fn load_commands(working_dir: &std::path::Path) -> Vec<CommandDefinition> {
    let mut commands = Vec::new();
    let mut loaded_names = std::collections::HashSet::new();

    // 1. Project-local
    let project_dir = working_dir.join(".libra").join("commands");
    load_commands_from_dir(&project_dir, &mut commands, &mut loaded_names);

    // 2. User-global
    if let Some(config_dir) = dirs::config_dir() {
        let user_dir = config_dir.join("libra").join("commands");
        load_commands_from_dir(&user_dir, &mut commands, &mut loaded_names);
    }

    // 3. Embedded defaults
    for cmd in load_embedded_commands() {
        if loaded_names.insert(cmd.name.clone()) {
            commands.push(cmd);
        }
    }

    commands
}

fn load_commands_from_dir(
    dir: &std::path::Path,
    commands: &mut Vec<CommandDefinition>,
    loaded_names: &mut std::collections::HashSet<String>,
) {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "md")
                && let Some(cmd) = super::parser::load_command_from_file(&path)
                && loaded_names.insert(cmd.name.clone())
            {
                commands.push(cmd);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_embedded_commands() {
        let commands = load_embedded_commands();
        assert_eq!(commands.len(), 4);
        let names: Vec<&str> = commands.iter().map(|c| c.name.as_str()).collect();
        assert!(names.contains(&"plan"));
        assert!(names.contains(&"code-review"));
        assert!(names.contains(&"verify"));
        assert!(names.contains(&"tdd"));
    }

    #[test]
    fn test_dispatch_plan() {
        let commands = load_embedded_commands();
        let dispatcher = CommandDispatcher::new(commands);

        let result = dispatcher.dispatch("/plan add user authentication");
        assert!(result.is_some());
        let result = result.unwrap();
        assert!(result.prompt.contains("add user authentication"));
        assert_eq!(result.agent.as_deref(), Some("planner"));
    }

    #[test]
    fn test_dispatch_code_review() {
        let commands = load_embedded_commands();
        let dispatcher = CommandDispatcher::new(commands);

        let result = dispatcher.dispatch("/code-review src/main.rs");
        assert!(result.is_some());
        let result = result.unwrap();
        assert!(result.prompt.contains("src/main.rs"));
        assert_eq!(result.agent.as_deref(), Some("code_reviewer"));
    }

    #[test]
    fn test_dispatch_no_args() {
        let commands = load_embedded_commands();
        let dispatcher = CommandDispatcher::new(commands);

        let result = dispatcher.dispatch("/verify");
        assert!(result.is_some());
        let result = result.unwrap();
        assert!(result.agent.is_none());
    }

    #[test]
    fn test_dispatch_unknown_command() {
        let commands = load_embedded_commands();
        let dispatcher = CommandDispatcher::new(commands);

        let result = dispatcher.dispatch("/unknown arg1 arg2");
        assert!(result.is_none());
    }

    #[test]
    fn test_dispatch_not_slash_command() {
        let commands = load_embedded_commands();
        let dispatcher = CommandDispatcher::new(commands);

        assert!(dispatcher.dispatch("hello world").is_none());
        assert!(dispatcher.dispatch("").is_none());
    }

    #[test]
    fn test_dispatch_get_by_name() {
        let commands = load_embedded_commands();
        let dispatcher = CommandDispatcher::new(commands);

        assert!(dispatcher.get("plan").is_some());
        assert!(dispatcher.get("nonexistent").is_none());
    }

    #[test]
    fn test_command_dispatch_with_agent_resolution() {
        // Integration test: dispatch a command and verify its agent can be found
        // via AgentRouter
        let commands = load_embedded_commands();
        let dispatcher = CommandDispatcher::new(commands);

        let agents = crate::internal::ai::agents::load_embedded_agents();
        let router = crate::internal::ai::agents::AgentRouter::new(agents);

        // Dispatch /plan command
        let result = dispatcher.dispatch("/plan implement user auth").unwrap();
        assert_eq!(result.agent.as_deref(), Some("planner"));

        // The agent referenced by the command should exist in the router
        let agent = router.get(result.agent.as_deref().unwrap()).unwrap();
        assert_eq!(agent.name, "planner");
        assert!(!agent.system_prompt.is_empty());
        assert!(!agent.tools.is_empty());

        // Dispatch /code-review command
        let result = dispatcher.dispatch("/code-review src/lib.rs").unwrap();
        assert_eq!(result.agent.as_deref(), Some("code_reviewer"));
        let agent = router.get("code_reviewer").unwrap();
        assert_eq!(agent.name, "code_reviewer");
    }

    #[test]
    fn test_command_without_agent_has_no_agent_resolution() {
        // Commands like /verify don't specify an agent
        let commands = load_embedded_commands();
        let dispatcher = CommandDispatcher::new(commands);

        let result = dispatcher.dispatch("/verify").unwrap();
        assert!(result.agent.is_none());
        // The prompt should still be expanded
        assert!(!result.prompt.is_empty());
    }

    #[test]
    fn test_load_commands_with_project_override() {
        let tmp = tempfile::TempDir::new().unwrap();
        let cmd_dir = tmp.path().join(".libra").join("commands");
        std::fs::create_dir_all(&cmd_dir).unwrap();
        std::fs::write(
            cmd_dir.join("plan.md"),
            "---\nname: plan\ndescription: Custom plan\nagent: custom_planner\n---\nCustom template $ARGUMENTS",
        ).unwrap();

        let commands = load_commands(tmp.path());
        let plan = commands.iter().find(|c| c.name == "plan").unwrap();
        assert_eq!(plan.description, "Custom plan");
        assert_eq!(plan.agent.as_deref(), Some("custom_planner"));
    }
}
