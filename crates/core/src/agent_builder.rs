use crate::llm::client::any::Client as AnyClient;
use alloc::sync::Arc;

use std::collections::HashMap;
use std::sync::Mutex;

use crate::agent::Agent;
use crate::agent::context::Context as AgentContext;
use crate::agent::delegation::Delegation;
use crate::agent::skills::SkillSet;
use crate::agent::sub_agents::SubAgents;
use crate::agent::tools;
use crate::agent::tools::delegate::Delegate as DelegateTool;
use crate::agent::tools::message::Messenger as MessageTool;
use crate::agent::tools::tool_set::ToolSet;
use crate::auth::store::Credentials as CredentialStore;
use crate::llm::provider::ModelSpec;
use crate::models::store::Models as ModelsStore;
use crate::persona::Repository as PersonaRepository;
use crate::persona::{Persona, Selection};
use crate::sync::MutexExt as _;
use crate::workspace::Workspace;

#[cfg(any(test, feature = "testing"))]
use crate::llm::client::fake::Client as FakeClient;

#[derive(Debug)]
/// Builds Agents from Personas.
pub struct AgentBuilder {
    context: AgentContext,
    delegation: Delegation,
    /// Replies every built Agent takes from, in tests in place of a provider.
    #[cfg(any(test, feature = "testing"))]
    fake_client: Option<FakeClient>,
    /// Whether each tool selection only reads. The answer depends on nothing else, so it is
    /// worked out once per selection.
    read_only: Mutex<HashMap<Selection, bool>>,
    skills: SkillSet,
    store: Arc<CredentialStore>,
    models: Arc<dyn ModelsStore>,
    workspace: Workspace,
}

impl AgentBuilder {
    /// Builds an Agent for `persona` with `tools`.
    fn assemble(&self, persona: &Persona, tools: ToolSet, skill_set: SkillSet) -> Agent {
        let context = persona
            .system_prompt
            .as_ref()
            .map_or_else(|| self.context.clone(), |prompt| self.context.clone().with_system_prompt(prompt));
        let agent = Agent::new(Arc::clone(&self.store))
            .with_name(&persona.name)
            .with_description(&persona.description)
            .with_tools(tools)
            .with_skills(skill_set)
            .with_context(&context);
        if let Some(model) = persona.model.as_ref()
            && let Some(provider_id) = model.provider.as_deref()
            && let Some(provider) = self.models.read(provider_id)
        {
            let mut model = model.clone();
            model.provider = None;
            agent.set_model(ModelSpec::new(provider, model));
        }
        if let Some(thinking) = persona.thinking {
            agent.set_thinking(thinking);
        }
        #[cfg(any(test, feature = "testing"))]
        let finished = self.with_test_client(agent);
        #[cfg(not(any(test, feature = "testing")))]
        let finished = agent;
        finished
    }

    /// A new Agent playing `persona`, with a Conversation and settings of its own. It has no
    /// delegation Tools, so a Sub-agent cannot start Sub-agents.
    #[inline]
    pub fn build(&self, persona: &Persona) -> Agent {
        self.assemble(persona, tools_for(&self.workspace, &persona.tools), skills_for(&self.skills, &persona.skills))
    }

    /// A Root agent playing `persona`: it has the `delegate` Tool, and a registry of its own
    /// Sub-agents. `personas` are the Personas it may choose from.
    #[expect(clippy::expect_used, reason = "`delegate` and `message` are not built-in tool names")]
    #[inline]
    /// A Root agent playing `persona`, with the delegation Tools.
    ///
    /// # Panics
    ///
    /// Panics if a delegation Tool name collides with a built-in one.
    pub fn build_root(self: Arc<Self>, persona: &Persona, personas: Arc<PersonaRepository>) -> Agent {
        let subs = Arc::new(SubAgents::new(self.delegation));
        let mut tools = tools_for(&self.workspace, &persona.tools);
        let skill_set = skills_for(&self.skills, &persona.skills);
        let delegate = DelegateTool::new(personas, Arc::clone(&self), Arc::clone(&subs), persona.clone());
        tools.register(delegate).expect("delegate is not a built-in tool name");
        tools
            .register(MessageTool::new(Arc::clone(&self), Arc::clone(&subs)))
            .expect("message is not a built-in tool name");
        self.assemble(persona, tools, skill_set).with_sub_agents(subs)
    }

    /// Whether every Tool the Persona may use only reads, so its calls may run at the same time.
    #[inline]
    pub fn is_read_only(&self, persona: &Persona) -> bool {
        let mut cache = self.read_only.lock_unpoisoned();
        *cache.entry(persona.tools.clone()).or_insert_with(|| {
            let tools = tools_for(&self.workspace, &persona.tools);
            tools.definitions().iter().all(|spec| tools.traits(&spec.name, "{}").read_only)
        })
    }

    #[inline]
    /// Creates a builder from what every Agent shares.
    pub fn new(
        store: Arc<CredentialStore>,
        models: Arc<dyn ModelsStore>,
        workspace: Workspace,
        context: AgentContext,
        skills: SkillSet,
    ) -> Self {
        Self {
            store,
            models,
            workspace,
            context,
            skills,
            delegation: Delegation::default(),
            read_only: Mutex::default(),
            #[cfg(any(test, feature = "testing"))]
            fake_client: None,
        }
    }

    /// How many tool selections have been checked for being read-only.
    #[cfg(test)]
    fn read_only_checks(&self) -> usize {
        self.read_only.lock_unpoisoned().len()
    }

    /// The limits on the Sub-agents of every Root agent this builder makes.
    #[inline]
    #[must_use]
    pub const fn with_delegation(mut self, limits: Delegation) -> Self {
        self.delegation = limits;
        self
    }

    #[cfg(any(test, feature = "testing"))]
    #[inline]
    #[must_use]
    pub fn with_fake_client(mut self, fake: FakeClient) -> Self {
        self.fake_client = Some(fake);
        self
    }

    #[cfg(any(test, feature = "testing"))]
    fn with_test_client(&self, agent: Agent) -> Agent {
        match self.fake_client.as_ref() {
            Some(fake) => agent.with_client(AnyClient::Fake(fake.clone())),
            None => agent,
        }
    }
}

fn tools_for(workspace: &Workspace, selection: &Selection) -> ToolSet {
    let mut tools = tools::builtin(workspace.clone());
    if let Selection::Only(ref names) = *selection {
        tools.retain(|name| names.iter().any(|allowed| allowed == name));
    }
    tools
}

fn skills_for(skill_set: &SkillSet, selection: &Selection) -> SkillSet {
    let mut skills = skill_set.clone();
    if let Selection::Only(ref names) = *selection {
        skills.retain(|skill| names.iter().any(|allowed| allowed == skill));
    }
    skills
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::client::StreamedCompletion;
    use crate::llm::client::any::Client as AnyClient;
    use crate::llm::client::fake::Client as FakeClient;
    use crate::llm::provider::{Model, ModelSpec, Provider};
    use crate::llm::thinking::Thinking;
    use crate::models::store::InMemory as InMemoryModelsStore;
    use crate::persona::Repository as PersonaRepository;
    use crate::test::temp_tree::TempTree;

    fn builder(tree: &TempTree) -> AgentBuilder {
        AgentBuilder::new(
            Arc::new(CredentialStore::in_memory()),
            Arc::new(InMemoryModelsStore::default()),
            tree.workspace(),
            AgentContext::new(tree.path()),
            SkillSet::default(),
        )
    }

    fn registry() -> Arc<PersonaRepository> {
        Arc::new(PersonaRepository::new())
    }

    fn root_store() -> Arc<CredentialStore> {
        Arc::new(CredentialStore::in_memory())
    }

    fn offered(agent: &Agent) -> Vec<String> {
        agent.tools().definitions().iter().map(|spec| spec.name.clone()).collect()
    }

    #[test]
    fn a_persona_gives_the_agent_its_name_and_description() {
        let tree = TempTree::new("builder-name");
        let agent = builder(&tree).build(&Persona::new("helper", "Helper", "helps"));
        assert_eq!((agent.name(), agent.description()), ("Helper", "helps"));
    }

    #[test]
    fn a_persona_selects_its_tools() {
        let tree = TempTree::new("builder-tools");
        let builder = builder(&tree);
        assert_eq!(offered(&builder.build(&Persona::new("a", "A", ""))).len(), 7);

        let explorer = builder.build(&explorer());
        assert_eq!(offered(&explorer), ["read_file", "list_directory", "search_files"]);
    }

    #[test]
    fn a_persona_can_set_its_own_model_and_thinking() {
        let tree = TempTree::new("builder-settings");
        let model = ModelSpec::new(Arc::new(Provider::openai()), Model::new("gpt-4.1"));
        let persona =
            Persona::new("a", "A", "").with_model(Model::reference("openai", "gpt-4.1")).with_thinking(Thinking::High);
        let agent = builder(&tree).build(&persona);
        assert_eq!(agent.model(), Some(model));
        assert_eq!(agent.thinking(), Thinking::High);
    }

    #[test]
    fn without_settings_a_persona_leaves_the_root_agents_to_be_inherited() {
        let tree = TempTree::new("builder-inherit");
        let agent = builder(&tree).build(&Persona::new("a", "A", ""));
        assert_eq!(agent.model(), None);
        assert_eq!(agent.thinking(), Thinking::default());
    }

    #[tokio::test]
    async fn a_persona_system_prompt_is_sent_with_the_conversation() {
        let tree = TempTree::new("builder-prompt");
        let fake = FakeClient::new(vec![Ok(StreamedCompletion { text: "ok".into(), ..Default::default() })]);
        let agent = builder(&tree)
            .build(
                &Persona::new("a", "A", "").with_system_prompt("be brief").with_model(Model::reference("ollama", "m")),
            )
            .with_client(AnyClient::Fake(fake.clone()));
        agent.respond("hi".into(), |_| {}).await.unwrap();
        assert_eq!(fake.requests()[0].0, 2, "the system prompt, then the prompt");
    }

    #[test]
    fn a_persona_is_read_only_when_every_tool_it_has_only_reads() {
        let tree = TempTree::new("builder-read-only");
        let builder = builder(&tree);
        assert!(!builder.is_read_only(&Persona::new("a", "A", "")));
        assert!(builder.is_read_only(&explorer()));
        assert!(builder.is_read_only(&Persona::new("none", "None", "").with_tools(Selection::Only(vec![]))));
    }

    #[test]
    fn the_read_only_answer_is_worked_out_once_per_tool_selection() {
        let tree = TempTree::new("builder-cache");
        let builder = builder(&tree);
        let explorer = explorer();
        assert_eq!(builder.read_only_checks(), 0);

        assert!(builder.is_read_only(&explorer));
        assert!(builder.is_read_only(&explorer));
        assert_eq!(builder.read_only_checks(), 1);

        // Another Persona with the same tools shares the answer, whatever its key.
        let same_tools = Persona::new("other", "Other", "").with_tools(explorer.tools);
        assert!(builder.is_read_only(&same_tools));
        assert_eq!(builder.read_only_checks(), 1);

        assert!(!builder.is_read_only(&Persona::new("a", "A", "")));
        assert_eq!(builder.read_only_checks(), 2);
    }

    #[test]
    fn a_root_agent_has_its_own_sub_agents_under_the_builders_limits() {
        let tree = TempTree::new("builder-root");
        let builder =
            Arc::new(builder(&tree).with_delegation(Delegation { max_per_session: 1, ..Delegation::default() }));
        let root =
            Arc::clone(&builder).build_root(&Persona::new("coding-assistant", "Coding Assistant", ""), registry());
        let tester = Persona::new("tester", "Tester", "");
        root.sub_agents().spawn(&tester, || Agent::new(root_store())).unwrap();
        root.sub_agents().spawn(&tester, || Agent::new(root_store())).unwrap_err();

        let other = builder.build_root(&Persona::new("secretary", "Secretary", ""), registry());
        assert!(other.sub_agents().infos().is_empty(), "each root agent keeps its own sub-agents");
    }

    #[tokio::test]
    async fn a_test_client_serves_every_agent_the_builder_makes() {
        let tree = TempTree::new("builder-fake");
        let fake = FakeClient::new(vec![Ok(StreamedCompletion { text: "hi".into(), ..Default::default() })]);
        let builder = Arc::new(builder(&tree).with_fake_client(fake.clone()));
        let persona = Persona::new("a", "A", "").with_model(Model::reference("ollama", "m"));
        let root = builder.build_root(&persona, registry());
        root.respond("hi".into(), |_| {}).await.unwrap();
        assert_eq!(fake.requests().len(), 1);
    }

    fn explorer() -> Persona {
        Persona::new("explorer", "Explorer", "Reads the Workspace.").with_tools(Selection::Only(vec![
            "read_file".into(),
            "list_directory".into(),
            "search_files".into(),
        ]))
    }
}
