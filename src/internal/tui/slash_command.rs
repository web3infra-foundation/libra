use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SlashCommand {
    Help,
    Clear,
    Model,
    Status,
    Quit,
}

impl SlashCommand {
    pub fn name(&self) -> &'static str {
        match self {
            SlashCommand::Help => "help",
            SlashCommand::Clear => "clear",
            SlashCommand::Model => "model",
            SlashCommand::Status => "status",
            SlashCommand::Quit => "quit",
        }
    }

    pub fn description(&self) -> &'static str {
        match self {
            SlashCommand::Help => "show available commands",
            SlashCommand::Clear => "clear conversation history",
            SlashCommand::Model => "show or change model",
            SlashCommand::Status => "show current status",
            SlashCommand::Quit => "quit the application",
        }
    }

    pub fn all() -> Vec<SlashCommand> {
        vec![
            SlashCommand::Help,
            SlashCommand::Clear,
            SlashCommand::Model,
            SlashCommand::Status,
            SlashCommand::Quit,
        ]
    }
}

pub fn parse_command(input: &str) -> Option<(SlashCommand, &str)> {
    let input = input.trim_start();
    if !input.starts_with('/') {
        return None;
    }

    let rest = &input[1..];
    let (name, args) = rest.split_once(' ').unwrap_or((rest, ""));
    let name = name.trim();

    for cmd in SlashCommand::all() {
        if cmd.name().eq_ignore_ascii_case(name) {
            return Some((cmd, args));
        }
    }

    None
}

pub fn get_commands_for_input(input: &str) -> Vec<(&'static str, &'static str)> {
    let input = input.trim_start();
    if !input.starts_with('/') {
        return Vec::new();
    }

    let search = input[1..].to_lowercase();
    SlashCommand::all()
        .iter()
        .filter(|c| c.name().contains(&search))
        .map(|c| (c.name(), c.description()))
        .collect()
}
