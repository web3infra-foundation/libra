//! Skill discovery from project and user skill directories.

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use super::{SkillDefinition, parse_skill_definition};

/// Load skills from project, user, then embedded tiers.
///
/// Project-local skills override user-global skills with the same name.
pub fn load_skills(working_dir: &Path) -> Vec<SkillDefinition> {
    let mut skills = Vec::new();
    let mut loaded_names = HashSet::new();

    let project_dir = working_dir.join(".libra").join("skills");
    load_skills_from_dir_with_seen(&project_dir, &mut skills, &mut loaded_names);

    if let Some(config_dir) = dirs::config_dir() {
        let user_dir = config_dir.join("libra").join("skills");
        load_skills_from_dir_with_seen(&user_dir, &mut skills, &mut loaded_names);
    }

    skills
}

/// Load every `*.md` skill in one directory.
pub fn load_skills_from_dir(dir: &Path) -> Vec<SkillDefinition> {
    let mut skills = Vec::new();
    let mut loaded_names = HashSet::new();
    load_skills_from_dir_with_seen(dir, &mut skills, &mut loaded_names);
    skills
}

fn load_skills_from_dir_with_seen(
    dir: &Path,
    skills: &mut Vec<SkillDefinition>,
    loaded_names: &mut HashSet<String>,
) {
    let mut paths = match std::fs::read_dir(dir) {
        Ok(entries) => entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "md"))
            .collect::<Vec<_>>(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Err(error) => {
            tracing::warn!(path = %dir.display(), error = %error, "failed to read skill directory");
            return;
        }
    };
    paths.sort();

    for path in paths {
        if let Some(skill) = load_skill_from_file(&path)
            && loaded_names.insert(skill.name.clone())
        {
            skills.push(skill);
        }
    }
}

fn load_skill_from_file(path: &Path) -> Option<SkillDefinition> {
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) => {
            tracing::warn!(path = %path.display(), error = %error, "failed to read skill file");
            return None;
        }
    };

    match parse_skill_definition(&content) {
        Ok(mut skill) => {
            skill.source_path = Some(PathBuf::from(path));
            Some(skill)
        }
        Err(error) => {
            tracing::warn!(path = %path.display(), error = %error, "failed to parse skill definition");
            None
        }
    }
}
