//! The Persona: what an Agent is asked to be.

use crate::domain::provider::Model;
use crate::domain::thinking::Thinking;
use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Deserialize, Default)]
/// Which Tools or skills a Persona may use.
pub enum Selection {
    #[default]
    All,
    Only(Vec<String>),
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
/// A named Agent prompt, with the model, thinking level and Tools it uses.
pub struct Persona {
    pub description: String,
    pub key: String,
    pub model: Option<Model>,
    pub name: String,
    pub system_prompt: Option<String>,
    pub thinking: Option<Thinking>,
    pub tools: Selection,
    #[serde(default)]
    pub skills: Selection,
}

impl Persona {
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
    pub fn with_model(mut self, model: Model) -> Self {
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

/// Whether `key` is a Persona key: a slug of lowercase letters, digits and hyphens.
#[must_use]
#[inline]
pub fn is_valid_key(key: &str) -> bool {
    !key.is_empty()
        && !key.starts_with('-')
        && key.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_lowercase_slugs() {
        for good in ["assistant", "coding-assistant", "agent2", "a-b-c"] {
            assert!(is_valid_key(good), "{good:?} should be valid");
        }
        for bad in ["", "-lead", "Has Space", "UPPER", "under_score", "tab\t"] {
            assert!(!is_valid_key(bad), "{bad:?} should be invalid");
        }
    }

    #[test]
    fn a_new_persona_has_all_tools_and_no_model() {
        let persona = Persona::new("tester", "Tester", "Tests.");
        assert_eq!(persona.tools, Selection::All);
        assert_eq!(persona.skills, Selection::All);
        assert_eq!(persona.model, None);
        assert_eq!(persona.system_prompt, None);
    }
}
