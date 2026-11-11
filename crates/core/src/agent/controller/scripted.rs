use alloc::collections::VecDeque;
use alloc::rc::Rc;
use core::cell::{Cell, RefCell};

use super::{Controller as AgentController, Reply};
use crate::agent::skills::SkillSummary;
use crate::agent::{Change as AgentChange, Settings};
use crate::llm::provider::{ModelSpec, Protocol, Provider};
use crate::llm::thinking::Thinking;

#[derive(Debug)]
/// An Agent controller that replays a script.
pub struct Controller {
    cleared: Cell<usize>,
    compactions: RefCell<Vec<Option<String>>>,
    interrupted: Cell<usize>,
    prompts: RefCell<Vec<String>>,
    replies: RefCell<VecDeque<Reply>>,
    settings: RefCell<Settings>,
    skills: RefCell<Vec<SkillSummary>>,
}

impl Controller {
    #[inline]
    pub const fn cleared(&self) -> usize {
        self.cleared.get()
    }

    #[inline]
    pub fn compactions(&self) -> Vec<Option<String>> {
        self.compactions.borrow().clone()
    }

    #[inline]
    pub const fn interrupted(&self) -> usize {
        self.interrupted.get()
    }

    #[inline]
    pub fn model(&self) -> Option<ModelSpec> {
        self.settings.borrow().model.clone()
    }

    #[must_use]
    #[inline]
    /// Creates a scripted Agent with a default model.
    pub fn new() -> Rc<Self> {
        Rc::new(Self {
            settings: RefCell::new(Settings {
                model: Some(ModelSpec::new(Provider::Ollama, Protocol::OpenAi, "bonsai-2:2b")),
                thinking: Thinking::default(),
            }),
            skills: RefCell::default(),
            prompts: RefCell::default(),
            compactions: RefCell::default(),
            replies: RefCell::default(),
            cleared: Cell::new(0),
            interrupted: Cell::new(0),
        })
    }

    #[inline]
    pub fn prompts(&self) -> Vec<String> {
        self.prompts.borrow().clone()
    }

    /// Queues a reply as the running turn would report it.
    #[inline]
    pub fn reply(&self, reply: Reply) {
        self.replies.borrow_mut().push_back(reply);
    }

    #[inline]
    pub fn with_skill(self: Rc<Self>, name: &str, description: &str) -> Rc<Self> {
        self.skills.borrow_mut().push(SkillSummary { name: name.into(), description: description.into() });
        self
    }

    #[inline]
    pub fn without_model(self: Rc<Self>) -> Rc<Self> {
        self.settings.borrow_mut().model = None;
        self
    }
}

impl AgentController for Controller {
    #[inline]
    fn apply(&self, change: AgentChange) {
        match change {
            AgentChange::SetModel(spec) => self.settings.borrow_mut().model = Some(spec),
            AgentChange::SetThinking(thinking) => {
                self.settings.borrow_mut().thinking = thinking;
            },
            AgentChange::ClearConversation => self.cleared.set(self.cleared.get().saturating_add(1)),
            AgentChange::Interrupt => self.interrupted.set(self.interrupted.get().saturating_add(1)),
        }
    }

    #[inline]
    fn compact(&self, focus: Option<String>) {
        self.compactions.borrow_mut().push(focus);
    }

    #[inline]
    fn settings(&self) -> Settings {
        self.settings.borrow().clone()
    }

    #[inline]
    fn skill_block(&self, name: &str) -> Option<String> {
        self.skills
            .borrow()
            .iter()
            .find(|skill| skill.name == name)
            .map(|skill| format!("<skill name=\"{}\">body</skill>", skill.name))
    }

    #[inline]
    fn skills(&self) -> Vec<SkillSummary> {
        self.skills.borrow().clone()
    }

    #[inline]
    fn start(&self, prompt: String) {
        self.prompts.borrow_mut().push(prompt);
    }

    #[inline]
    fn try_next(&self) -> Option<Reply> {
        self.replies.borrow_mut().pop_front()
    }
}

impl<T: AgentController + ?Sized> AgentController for Rc<T> {
    #[inline]
    fn apply(&self, change: AgentChange) {
        (**self).apply(change);
    }
    #[inline]
    fn compact(&self, focus: Option<String>) {
        (**self).compact(focus);
    }
    #[inline]
    fn settings(&self) -> Settings {
        (**self).settings()
    }
    #[inline]
    fn skill_block(&self, name: &str) -> Option<String> {
        (**self).skill_block(name)
    }
    #[inline]
    fn skills(&self) -> Vec<SkillSummary> {
        (**self).skills()
    }
    #[inline]
    fn start(&self, prompt: String) {
        (**self).start(prompt);
    }
    #[inline]
    fn try_next(&self) -> Option<Reply> {
        (**self).try_next()
    }
}
