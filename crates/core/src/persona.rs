use crate::config::dir;
use crate::fsutil::read_markdown_with_frontmatter;
use crate::llm::provider::ModelSpec;
use crate::llm::thinking::Thinking;
use alloc::collections::BTreeMap;
use core::error;
use core::fmt;
use serde::Deserialize;
use serde_saphyr::from_str;
use std::io;
use std::path::{Path, PathBuf};
use tokio::fs;

/// The Persona written to disk when the personas folder holds none, so it can be copied and edited.
const ASSISTANT_FILE: &str = "assistant.md";
/// The built-in assistant Persona.
const ASSISTANT: &str = include_str!("persona/assistant.md");

#[derive(Debug, Clone, PartialEq, Eq, Hash, Deserialize, Default)]
pub enum Selection {
    #[default]
    All,
    Only(Vec<String>),
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Persona {
    pub description: String,
    pub key: String,
    pub model: Option<ModelSpec>,
    pub name: String,
    pub system_prompt: Option<String>,
    pub thinking: Option<Thinking>,
    pub tools: Selection,
    #[serde(default)]
    pub skills: Selection,
}

impl Persona {
    #[inline]
    /// Reads a Persona from a Markdown file with YAML frontmatter.
    ///
    /// # Errors
    ///
    /// Fails if the file cannot be read or its frontmatter is missing or invalid.
    pub async fn from_markdown_file(path: &Path) -> Result<Self, Failure> {
        let (frontmatter, content) = read_markdown_with_frontmatter(path)
            .await
            .map_err(|e| Failure::Io { path: path.to_path_buf(), source: e })?;

        let header = frontmatter.ok_or_else(|| Failure::MissingFrontmatter(path.to_path_buf()))?;

        let persona: Self = from_str(&header)
            .map_err(|e| Failure::InvalidFrontmatter { path: path.to_path_buf(), source: Box::new(e) })?;

        let body = content.trim();
        if body.is_empty() { Ok(persona) } else { Ok(persona.with_system_prompt(body)) }
    }

    #[must_use]
    #[inline]
    /// Creates a Persona with no prompt, all Tools and no model.
    pub fn new(key: &str, name: &str, description: &str) -> Self {
        Self {
            key: key.to_owned(),
            name: name.to_owned(),
            description: description.to_owned(),
            system_prompt: None,
            tools: Selection::All,
            model: None,
            thinking: None,
            skills: Selection::All,
        }
    }

    #[must_use]
    #[inline]
    pub fn with_model(mut self, model: ModelSpec) -> Self {
        self.model = Some(model);
        self
    }

    #[must_use]
    #[inline]
    pub fn with_system_prompt(mut self, prompt: &str) -> Self {
        self.system_prompt = Some(prompt.to_owned());
        self
    }

    #[must_use]
    #[inline]
    pub const fn with_thinking(mut self, thinking: Thinking) -> Self {
        self.thinking = Some(thinking);
        self
    }

    #[must_use]
    #[inline]
    pub fn with_tools(mut self, tools: Selection) -> Self {
        self.tools = tools;
        self
    }

    #[must_use]
    #[inline]
    pub fn with_skills(mut self, skills: Selection) -> Self {
        self.skills = skills;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// A Persona's key and description.
pub struct Info {
    pub description: String,
    pub key: String,
    pub name: String,
}

#[derive(Debug)]
/// Why a Persona failed to load.
pub enum Failure {
    DuplicateKey(String),
    InvalidFrontmatter { path: PathBuf, source: Box<serde_saphyr::Error> },
    InvalidKey(String),
    Io { path: PathBuf, source: io::Error },
    MissingFrontmatter(PathBuf),
}

impl fmt::Display for Failure {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::InvalidKey(ref key) => {
                write!(f, "`{key}` is not a Persona key: use lowercase letters, digits and hyphens")
            },
            Self::DuplicateKey(ref key) => {
                write!(f, "a Persona with key `{key}` already exists")
            },
            Self::Io { ref path, ref source } => {
                write!(f, "failed to read `{}`: {}", path.display(), source)
            },
            Self::MissingFrontmatter(ref path) => {
                write!(f, "missing frontmatter in `{}`", path.display())
            },
            Self::InvalidFrontmatter { ref path, ref source } => {
                write!(f, "invalid frontmatter in `{}`: {}", path.display(), source)
            },
        }
    }
}

impl error::Error for Failure {
    #[inline]
    fn cause(&self) -> Option<&dyn error::Error> {
        None
    }

    #[inline]
    fn description(&self) -> &'static str {
        "a Persona could not be loaded"
    }

    #[inline]
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        None
    }
}

#[derive(Default, Debug)]
/// The Personas on disk.
pub struct Repository {
    personas: BTreeMap<String, Persona>,
}

impl Repository {
    #[inline]
    /// Adds a Persona. The key must be valid and not already used.
    ///
    /// # Errors
    ///
    /// Fails if the key is invalid or already used.
    pub fn add(&mut self, persona: Persona) -> Result<(), Failure> {
        if !is_valid_key(&persona.key) {
            return Err(Failure::InvalidKey(persona.key));
        }
        if self.personas.contains_key(&persona.key) {
            return Err(Failure::DuplicateKey(persona.key));
        }
        drop(self.personas.insert(persona.key.clone(), persona));
        Ok(())
    }

    #[must_use]
    #[inline]
    pub fn get(&self, key: &str) -> Option<&Persona> {
        self.personas.get(key)
    }

    #[must_use]
    #[inline]
    /// The key, name and description of each Persona.
    pub fn infos(&self) -> Vec<Info> {
        self.personas
            .values()
            .map(|persona| Info {
                key: persona.key.clone(),
                name: persona.name.clone(),
                description: persona.description.clone(),
            })
            .collect()
    }

    #[inline]
    /// Loads the Personas from the config directory.
    ///
    /// # Errors
    ///
    /// Fails if the config directory is missing or a Persona file is invalid.
    pub async fn load(&mut self) -> Result<(), Failure> {
        let config_dir = dir().ok_or_else(|| Failure::Io {
            path: PathBuf::new(),
            source: io::Error::new(io::ErrorKind::NotFound, "config directory not found"),
        })?;

        self.load_from(&config_dir.join("personas")).await
    }

    #[inline]
    /// Loads the Personas from Markdown files in `source_dir`, creating it if missing.
    ///
    /// # Errors
    ///
    /// Fails if the directory cannot be created or read, or a Persona file is invalid.
    pub async fn load_from(&mut self, source_dir: &Path) -> Result<(), Failure> {
        let personas_dir = source_dir.to_path_buf();

        fs::create_dir_all(&personas_dir).await.map_err(|source| Failure::Io { path: personas_dir.clone(), source })?;

        let mut entries =
            fs::read_dir(&personas_dir).await.map_err(|source| Failure::Io { path: personas_dir.clone(), source })?;

        let mut paths = Vec::new();
        while let Some(entry) =
            entries.next_entry().await.map_err(|source| Failure::Io { path: personas_dir.clone(), source })?
        {
            let path = entry.path();

            if path.is_file()
                && path.extension().and_then(|ext| ext.to_str()).is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
            {
                paths.push(path);
            }
        }

        if paths.is_empty() {
            let path = personas_dir.join(ASSISTANT_FILE);
            fs::write(&path, ASSISTANT).await.map_err(|source| Failure::Io { path: path.clone(), source })?;
            paths.push(path);
        }

        for path in paths {
            let persona = Persona::from_markdown_file(&path).await?;
            self.add(persona)?;
        }

        Ok(())
    }

    #[must_use]
    #[inline]
    pub fn new() -> Self {
        Self::default()
    }
}

fn is_valid_key(key: &str) -> bool {
    !key.is_empty()
        && !key.starts_with('-')
        && key.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::provider::{Protocol, Provider};
    use crate::test::temp_tree::TempTree;
    use std::fs;

    #[test]
    fn personas_are_listed_by_key() {
        let mut registry = Repository::new();
        registry.add(Persona::new("zeta", "Zeta", "last")).unwrap();
        registry.add(Persona::new("alpha", "Alpha", "first")).unwrap();
        let keys: Vec<String> = registry.infos().into_iter().map(|p| p.key).collect();
        assert_eq!(keys, ["alpha", "zeta"]);
        assert_eq!(registry.get("alpha").unwrap().name, "Alpha");
        assert!(registry.get("missing").is_none());
    }

    #[test]
    fn keys_must_be_unique_readable_slugs() {
        let mut registry = Repository::new();
        registry.add(Persona::new("tester", "T", "")).unwrap();
        assert!(matches!(
            registry.add(Persona::new("tester", "Again", "")),
            Err(Failure::DuplicateKey(key)) if key == "tester"
        ));
        for bad in ["", "-lead", "Has Space", "UPPER", "under_score", "tab\t"] {
            assert!(
                matches!(
                    registry.add(Persona::new(bad, "x", "")),
                    Err(Failure::InvalidKey(key)) if key == bad
                ),
                "{bad:?}"
            );
        }
    }

    #[tokio::test]
    async fn every_markdown_file_in_the_folder_is_a_persona() {
        let tree = TempTree::new("personas-load")
            .file(
                "personas/coding-assistant.md",
                "---\nkey: coding-assistant\nname: Coding Assistant\ndescription: Writes code.\ntools: All\n---\n",
            )
            .file(
                "personas/explorer.md",
                "---\nkey: explorer\nname: Explorer\ndescription: Reads.\ntools:\n  Only:\n    - read_file\n---\n\nYou only read.\n",
            )
            .file("personas/notes.txt", "not a persona");
        let mut registry = Repository::new();
        registry.load_from(&tree.path().join("personas")).await.unwrap();

        let keys: Vec<String> = registry.infos().into_iter().map(|p| p.key).collect();
        assert_eq!(keys, ["coding-assistant", "explorer"]);

        let coding = registry.get("coding-assistant").unwrap();
        assert_eq!(coding.tools, Selection::All);
        assert_eq!(coding.system_prompt, None);

        let explorer = registry.get("explorer").unwrap();
        assert_eq!(explorer.tools, Selection::Only(vec!["read_file".into()]));
        assert_eq!(explorer.system_prompt.as_deref(), Some("You only read."));
    }

    #[tokio::test]
    async fn an_empty_folder_gets_the_assistant_persona_on_disk() {
        let tree = TempTree::new("personas-empty").file("personas/notes.txt", "not a persona");
        let folder = tree.path().join("personas");
        let mut registry = Repository::new();
        registry.load_from(&folder).await.unwrap();

        let written = fs::read_to_string(folder.join("assistant.md")).unwrap();
        assert_eq!(written, ASSISTANT);
        let keys: Vec<String> = registry.infos().into_iter().map(|p| p.key).collect();
        assert_eq!(keys, ["assistant"]);
        let assistant = registry.get("assistant").unwrap();
        assert_eq!(assistant.name, "Assistant");
        assert_eq!(assistant.tools, Selection::All);
        assert!(assistant.system_prompt.as_deref().is_some_and(|prompt| prompt.starts_with("You are Assistant,")));
    }

    #[tokio::test]
    async fn a_missing_folder_is_created_with_the_assistant_persona() {
        let tree = TempTree::new("personas-missing");
        let folder = tree.path().join("personas");
        let mut registry = Repository::new();
        registry.load_from(&folder).await.unwrap();

        assert!(folder.join("assistant.md").is_file());
        assert!(registry.get("assistant").is_some());
    }

    #[tokio::test]
    async fn a_folder_with_personas_gets_no_assistant_persona() {
        let tree = TempTree::new("personas-configured")
            .file("personas/coding.md", "---\nkey: coding\nname: Coding\ndescription: Codes.\ntools: All\n---\n");
        let folder = tree.path().join("personas");
        let mut registry = Repository::new();
        registry.load_from(&folder).await.unwrap();

        let keys: Vec<String> = registry.infos().into_iter().map(|p| p.key).collect();
        assert_eq!(keys, ["coding"]);
        assert!(!folder.join("assistant.md").exists());
    }

    #[tokio::test]
    async fn a_persona_file_can_set_its_model_and_thinking() {
        let tree = TempTree::new("personas-model").file(
            "personas/tester.md",
            "---\nkey: tester\nname: Tester\ndescription: Tests.\ntools: All\nmodel:\n  provider: Ollama\n  protocol: openai\n  id: qwen\n  context_limit: 8192\nthinking: High\n---\nWrite tests.\n",
        );
        let mut registry = Repository::new();
        registry.load_from(&tree.path().join("personas")).await.unwrap();

        let tester = registry.get("tester").unwrap();
        assert_eq!(
            tester.model,
            Some(ModelSpec { context_limit: Some(8192), ..ModelSpec::new(Provider::Ollama, Protocol::OpenAi, "qwen") })
        );
        assert_eq!(tester.thinking, Some(Thinking::High));
    }

    #[tokio::test]
    async fn a_file_without_frontmatter_is_an_error() {
        let tree = TempTree::new("personas-bare").file("personas/bare.md", "Just text.\n");
        let mut registry = Repository::new();
        let error = registry.load_from(&tree.path().join("personas")).await.unwrap_err();
        assert!(matches!(error, Failure::MissingFrontmatter(path) if path.ends_with("bare.md")));
    }

    #[tokio::test]
    async fn a_file_with_an_invalid_key_is_an_error() {
        let tree = TempTree::new("personas-key")
            .file("personas/bad.md", "---\nkey: Not A Slug\nname: Bad\ndescription: \"\"\ntools: All\n---\n");
        let mut registry = Repository::new();
        let error = registry.load_from(&tree.path().join("personas")).await.unwrap_err();
        assert!(matches!(error, Failure::InvalidKey(key) if key == "Not A Slug"));
    }
}
