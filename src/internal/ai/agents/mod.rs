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

pub use parser::AgentDefinition;
pub use router::{AgentRouter, load_agents, load_embedded_agents};
