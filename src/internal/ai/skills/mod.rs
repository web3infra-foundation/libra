//! Markdown skill loading and dispatch.
//!
//! Skills are reusable workflow fragments stored as markdown files with TOML
//! frontmatter. They are distinct from slash commands: commands are one-shot
//! prompt expansions, while skills may constrain tool policy and carry audit
//! metadata such as version and checksum.
//!
//! Loading follows a three-tier hierarchy (highest priority wins on name conflict):
//! 1. Project-local (`.libra/skills/*.md`)
//! 2. User-global (`~/.config/libra/skills/*.md`)
//! 3. Embedded defaults (compiled into the binary, e.g. the built-in "libra" skill)

pub mod dispatcher;
pub mod loader;
pub mod parser;
pub mod scanner;

pub use dispatcher::{SkillDispatchResult, SkillDispatcher};
pub use loader::{load_embedded_skills, load_skills, load_skills_from_dir};
pub use parser::{SkillDefinition, SkillParseError, parse_skill_definition};
pub use scanner::{SkillScanSeverity, SkillScanWarning, scan_skill};
