//! Agent definition system for specialized AI agents.
//!
//! Agents are defined in markdown files with YAML frontmatter. Each agent has a
//! name, description, tool list, model preference, and system prompt. The agent
//! router auto-selects the appropriate agent based on user input.
//!
//! Agent definitions are loaded from a three-tier hierarchy:
//! 1. `{working_dir}/.libra/agents/*.md` (project-local)
//! 2. `~/.config/libra/agents/*.md` (user-global)
//! 3. Embedded defaults compiled into the binary

pub mod parser;
pub mod router;

pub use parser::{AgentProfile, parse_agent_profile};
pub use router::{AgentProfileRouter, load_embedded_profiles, load_profiles};

#[deprecated(note = "Use AgentProfileRouter instead.")]
pub type AgentRouter = AgentProfileRouter;

#[deprecated(note = "Use load_profiles instead.")]
pub fn load_agents(working_dir: &std::path::Path) -> Vec<AgentProfile> {
    load_profiles(working_dir)
}

#[deprecated(note = "Use load_embedded_profiles instead.")]
pub fn load_embedded_agents() -> Vec<AgentProfile> {
    load_embedded_profiles()
}
