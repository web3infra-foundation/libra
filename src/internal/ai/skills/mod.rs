//! Markdown skill loading and dispatch.
//!
//! Skills are reusable workflow fragments stored as markdown files with TOML
//! frontmatter. They are distinct from slash commands: commands are one-shot
//! prompt expansions, while skills may constrain tool policy and carry audit
//! metadata such as version and checksum.

pub mod dispatcher;
pub mod loader;
pub mod parser;
pub mod scanner;

pub use dispatcher::{SkillDispatchResult, SkillDispatcher};
pub use loader::{load_skills, load_skills_from_dir};
pub use parser::{SkillDefinition, SkillParseError, parse_skill_definition};
pub use scanner::{SkillScanSeverity, SkillScanWarning, scan_skill};
