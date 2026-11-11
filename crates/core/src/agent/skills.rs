use anyhow::{Context as _, Result, bail};
use core::fmt;
use core::fmt::{Display, Formatter};
use serde::Deserialize;
use serde_saphyr::from_str;
use std::fs;
use std::path::{Path, PathBuf};

/// Where skills are looked for under each root.
const DEFAULT_SKILL_DIRS: &[&str] =
    &[".agents/skills", ".claude/skills", ".codex/skills", ".pi/agent/skills", ".config/thoth/skills"];

/// Directories that are never searched for skills.
const DIRS_TO_IGNORE: &[&str] = &[".git", "node_modules", "target", "build"];

#[derive(Debug, Clone, Deserialize)]
/// A skill: instructions an Agent can load.
pub struct Skill {
    #[serde(skip)]
    pub body: String,
    pub description: String,
    pub name: String,
    #[serde(skip)]
    pub path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// A skill's name and description.
pub struct SkillSummary {
    pub description: String,
    pub name: String,
}

impl Skill {
    #[must_use]
    #[inline]
    /// The skill as a block of text for the prompt.
    pub fn block(&self) -> String {
        let dir = self.path.parent().map(|dir| dir.display().to_string()).unwrap_or_default();
        format!(
            "<skill name=\"{}\" location=\"{}\">\nReferences are relative to {dir}.\n\n{}\n</skill>",
            self.name,
            self.path.display(),
            self.body,
        )
    }

    #[must_use]
    #[inline]
    pub fn description(&self) -> &str {
        &self.description
    }

    #[inline]
    /// Reads a skill from a Markdown file with YAML frontmatter.
    ///
    /// # Errors
    ///
    /// Fails if the file cannot be read or its frontmatter is missing or invalid.
    pub fn from_file(path: &Path) -> Result<Self> {
        let content = fs::read_to_string(path).context(format!("Failed to read skill file: {}", path.display()))?;

        let after_opening = content
            .strip_prefix("---")
            .with_context(|| format!("Skill file must start with '---' delimiter: {}", path.display()))?;
        let inside =
            after_opening.strip_prefix('\n').or_else(|| after_opening.strip_prefix("\r\n")).with_context(|| {
                format!("Skill file must have a newline after the opening '---' delimiter: {}", path.display())
            })?;

        let (metadata_str, after) = inside
            .split_once("\n---")
            .with_context(|| format!("Skill file must have a closing '---' delimiter: {}", path.display()))?;

        if !after.starts_with('\n') && !after.starts_with("\r\n") && !after.is_empty() {
            bail!("Closing delimiter '---' must be on a line by itself in skill file: {}", path.display());
        }

        let mut skill: Self =
            from_str(metadata_str).context(format!("Failed to parse skill metadata in file: {}", path.display()))?;
        skill.path = path.to_path_buf();
        after.trim().clone_into(&mut skill.body);

        Ok(skill)
    }

    #[must_use]
    #[inline]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    #[inline]
    pub fn summary(&self) -> SkillSummary {
        SkillSummary { name: self.name.clone(), description: self.description.clone() }
    }
}

#[derive(Default, Clone, Debug)]
/// The skills found in the Workspace.
pub struct SkillSet {
    skills: Vec<Skill>,
}

impl Display for SkillSet {
    #[inline]
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        for (i, skill) in self.skills.iter().enumerate() {
            if i > 0 {
                writeln!(f)?;
            }

            write!(f, "{}: {}", skill.name, skill.description)?;
        }

        Ok(())
    }
}

impl SkillSet {
    #[inline]
    /// Loads the skills under each root's default skill directories.
    ///
    /// # Errors
    ///
    /// Fails if a skill file is invalid.
    pub fn from_default_dirs(roots: &[&Path]) -> Result<Self> {
        let mut skills: Vec<Skill> = Vec::new();
        for root in roots {
            for dir in DEFAULT_SKILL_DIRS {
                let path = root.join(dir);
                if !path.is_dir() {
                    continue;
                }
                let mut found = Vec::new();
                load_skills_recursive(&path, &mut found)
                    .with_context(|| format!("Failed to load skills from directory: {}", path.display()))?;
                for skill in found {
                    if !skills.iter().any(|existing| existing.name == skill.name) {
                        skills.push(skill);
                    }
                }
            }
        }

        Ok(Self { skills })
    }

    #[must_use]
    #[inline]
    pub fn get_skill_by_name(&self, name: &str) -> Option<&Skill> {
        self.skills.iter().find(|skill| skill.name.eq_ignore_ascii_case(name))
    }

    #[inline]
    pub fn iter(&self) -> impl Iterator<Item = &Skill> {
        self.skills.iter()
    }

    #[inline]
    pub fn push(&mut self, skill: Skill) {
        self.skills.push(skill);
    }

    #[must_use]
    #[inline]
    pub fn summaries(&self) -> Vec<SkillSummary> {
        self.skills.iter().map(Skill::summary).collect()
    }

    #[inline]
    pub fn retain<Callback>(&mut self, keep: Callback)
    where
        Callback: Fn(&str) -> bool,
    {
        self.skills.retain(|skill| keep(skill.name()));
    }
}

#[inline]
/// Loads every skill under `dir`.
///
/// # Errors
///
/// Fails if a skill file is invalid.
pub fn load_skills_from_dir(dir: &Path) -> Result<SkillSet> {
    let mut skills = Vec::new();
    load_skills_recursive(dir, &mut skills)?;
    Ok(SkillSet { skills })
}

/// Collects skills below `dir`, skipping directories such as `.git`.
fn load_skills_recursive(dir: &Path, skills: &mut Vec<Skill>) -> Result<()> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Ok(());
    };

    for result in entries {
        let Ok(entry) = result else {
            continue;
        };

        let path = entry.path();

        if path.is_dir() && !DIRS_TO_IGNORE.iter().any(|ignore| path.ends_with(ignore)) {
            let _unused = load_skills_recursive(&path, skills);
        } else if path.is_file()
            && path.file_stem().is_some_and(|name| name == "SKILL")
            && path.extension().is_some_and(|ext| ext == "md")
        {
            let skill = Skill::from_file(&path).with_context(|| format!("Failed to load skill: {}", path.display()))?;

            skills.push(skill);
        } else {
            // Anything else is not a skill.
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::process;

    #[test]
    fn loads_skills_with_only_name_and_description_from_default_dirs_under_each_root() {
        let root = env::temp_dir().join(format!("{}-skills-{}", crate::APP_NAME, process::id()));
        let dir = root.join(".agents/skills/tdd");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("SKILL.md"), "---\nname: tdd\ndescription: Test first\n---\n\n# Body\n").unwrap();

        let skills = SkillSet::from_default_dirs(&[root.as_path(), root.as_path()]).unwrap();
        fs::remove_dir_all(&root).unwrap();

        assert_eq!(skills.iter().map(|s| s.name().to_owned()).collect::<Vec<_>>(), ["tdd"]);
    }

    #[test]
    fn a_malformed_skill_file_is_an_error_naming_the_file() {
        let dir = env::temp_dir().join(format!("{}-bad-skill-{}", crate::APP_NAME, process::id()));
        fs::create_dir_all(&dir).unwrap();
        for (name, text) in [
            ("no-open.md", "name: x\n"),
            ("no-newline.md", "---name: x\n---\n"),
            ("no-close.md", "---\nname: x\ndescription: y\n"),
            ("close-not-alone.md", "---\nname: x\n---oops\n"),
        ] {
            let path = dir.join(name);
            fs::write(&path, text).unwrap();
            let error = Skill::from_file(&path).unwrap_err().to_string();
            assert!(error.contains(name), "{error}");
        }
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_skill_block_carries_its_location_and_body() {
        let root = env::temp_dir().join(format!("{}-skill-block-{}", crate::APP_NAME, process::id()));
        let dir = root.join(".agents/skills/tdd");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("SKILL.md"), "---\nname: tdd\ndescription: Test first\n---\n\n# Body\n").unwrap();

        let skills = SkillSet::from_default_dirs(&[root.as_path()]).unwrap();
        let block = skills.get_skill_by_name("TDD").unwrap().block();
        fs::remove_dir_all(&root).unwrap();

        assert!(block.starts_with("<skill name=\"tdd\" location=\""), "{block}");
        assert!(block.contains("SKILL.md\">\nReferences are relative to "), "{block}");
        assert!(block.ends_with("\n\n# Body\n</skill>"), "{block}");
    }
}
