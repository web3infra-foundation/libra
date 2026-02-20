//! Slash command system.
//!
//! Commands are `/name arguments` patterns intercepted from user input.
//! Each command is defined in a markdown file with YAML frontmatter specifying
//! its name, description, optional agent, and a template body with `$ARGUMENTS`
//! placeholder.
//!
//! Command definitions are loaded from a three-tier hierarchy:
//! 1. `{working_dir}/.libra/commands/*.md` (project-local)
//! 2. `~/.config/libra/commands/*.md` (user-global)
//! 3. Embedded defaults compiled into the binary

pub mod dispatcher;
pub mod parser;

pub use dispatcher::{CommandDispatcher, DispatchResult, load_commands, load_embedded_commands};
pub use parser::CommandDefinition;
