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

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    fn write_skill(dir: &Path, filename: &str, content: &str) {
        std::fs::write(dir.join(filename), content).expect("write skill");
    }

    /// `load_skills_from_dir` returns an empty vec for a non-existent
    /// directory — the loader is fault-tolerant, not an error path.
    #[test]
    fn load_skills_from_dir_returns_empty_for_missing_directory() {
        let tmp = TempDir::new().expect("tmp dir");
        let missing = tmp.path().join("never-created");
        assert!(load_skills_from_dir(&missing).is_empty());
    }

    /// Empty directory → empty Vec.
    #[test]
    fn load_skills_from_dir_returns_empty_for_empty_directory() {
        let tmp = TempDir::new().expect("tmp dir");
        let dir = tmp.path().join("skills");
        std::fs::create_dir_all(&dir).expect("mkdir");
        assert!(load_skills_from_dir(&dir).is_empty());
    }

    /// `load_skills_from_dir` parses all `.md` files in the directory
    /// and returns them sorted by path.
    #[test]
    fn load_skills_from_dir_parses_md_files_sorted() {
        let tmp = TempDir::new().expect("tmp dir");
        let dir = tmp.path().join("skills");
        std::fs::create_dir_all(&dir).expect("mkdir");

        write_skill(&dir, "zebra.md", "---\nname = \"zebra\"\n---\nBody Z");
        write_skill(&dir, "alpha.md", "---\nname = \"alpha\"\n---\nBody A");
        write_skill(&dir, "mid.md", "---\nname = \"mid\"\n---\nBody M");

        let skills = load_skills_from_dir(&dir);
        assert_eq!(skills.len(), 3);
        // Sorted by path → alpha.md, mid.md, zebra.md.
        assert_eq!(skills[0].name, "alpha");
        assert_eq!(skills[1].name, "mid");
        assert_eq!(skills[2].name, "zebra");
    }

    /// Non-`.md` files are silently skipped — the extension filter
    /// keeps the loader focused on markdown skills.
    #[test]
    fn load_skills_from_dir_skips_non_markdown_extensions() {
        let tmp = TempDir::new().expect("tmp dir");
        let dir = tmp.path().join("skills");
        std::fs::create_dir_all(&dir).expect("mkdir");

        write_skill(&dir, "real.md", "---\nname = \"real\"\n---\nBody");
        std::fs::write(dir.join("readme.txt"), "not a skill").expect("write txt");
        std::fs::write(dir.join("config.toml"), "name=foo").expect("write toml");
        std::fs::write(dir.join("script.sh"), "#!/bin/sh").expect("write sh");

        let skills = load_skills_from_dir(&dir);
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "real");
    }

    /// Malformed skill files (missing frontmatter, invalid TOML, etc.)
    /// are silently dropped: the loader logs a warning and continues.
    /// Pin the fault-tolerance so a single bad file doesn't poison
    /// the whole load.
    #[test]
    fn load_skills_from_dir_drops_malformed_skills_silently() {
        let tmp = TempDir::new().expect("tmp dir");
        let dir = tmp.path().join("skills");
        std::fs::create_dir_all(&dir).expect("mkdir");

        write_skill(&dir, "good.md", "---\nname = \"good\"\n---\nBody");
        // Missing frontmatter — should be skipped, not abort the load.
        write_skill(&dir, "bad-no-fence.md", "just body");
        // Invalid TOML in frontmatter — also skipped.
        write_skill(&dir, "bad-toml.md", "---\nname = \nstray\n---\nBody");
        // Empty name — skipped (MissingName).
        write_skill(&dir, "bad-empty-name.md", "---\nname = \"\"\n---\nBody");

        let skills = load_skills_from_dir(&dir);
        assert_eq!(
            skills.len(),
            1,
            "only the valid skill must survive; got: {:?}",
            skills.iter().map(|s| &s.name).collect::<Vec<_>>(),
        );
        assert_eq!(skills[0].name, "good");
    }

    /// Loaded skills carry their source path so audit / debug
    /// surfaces can locate the file.
    #[test]
    fn load_skills_from_dir_populates_source_path() {
        let tmp = TempDir::new().expect("tmp dir");
        let dir = tmp.path().join("skills");
        std::fs::create_dir_all(&dir).expect("mkdir");

        let path = dir.join("with-path.md");
        std::fs::write(&path, "---\nname = \"x\"\n---\nBody").expect("write");

        let skills = load_skills_from_dir(&dir);
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].source_path.as_deref(), Some(path.as_path()));
    }

    /// Duplicate-name skills: the first-encountered (sorted-by-path)
    /// wins; later same-name skills are silently dropped via the
    /// `loaded_names` HashSet. Pin this dedup rule so the
    /// project-overrides-user precedence (in `load_skills`) stays
    /// observable here.
    #[test]
    fn load_skills_from_dir_dedupes_by_name_keeping_first_sorted() {
        let tmp = TempDir::new().expect("tmp dir");
        let dir = tmp.path().join("skills");
        std::fs::create_dir_all(&dir).expect("mkdir");

        // Sorted order: alpha.md, beta.md. Both declare name=`shared`.
        write_skill(
            &dir,
            "alpha.md",
            "---\nname = \"shared\"\n---\nBody from alpha",
        );
        write_skill(
            &dir,
            "beta.md",
            "---\nname = \"shared\"\n---\nBody from beta",
        );

        let skills = load_skills_from_dir(&dir);
        assert_eq!(skills.len(), 1, "duplicates by name must collapse");
        // alpha.md sorts first → wins.
        assert_eq!(skills[0].template, "Body from alpha");
        assert!(
            skills[0]
                .source_path
                .as_ref()
                .unwrap()
                .ends_with("alpha.md"),
            "first-sorted source path must survive",
        );
    }

    /// `load_skill_from_file` handles unreadable / non-existent paths
    /// by logging a warning and returning `None`. Pin both branches
    /// so the loader never panics on filesystem hiccups.
    #[test]
    fn load_skill_from_file_returns_none_for_missing_file() {
        let tmp = TempDir::new().expect("tmp dir");
        let missing = tmp.path().join("never-created.md");
        assert!(load_skill_from_file(&missing).is_none());
    }
}
