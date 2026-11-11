use super::client::Client;
use thoth_core::builtin::skills::SKILL_PREFIX;
use thoth_core::llm::provider::Provider;
use thoth_core::llm::thinking::Thinking;
use thoth_core::session::{LoginState, Refusal};

#[derive(Debug, Clone, PartialEq, Eq)]
/// A slash command.
pub struct Command {
    pub description: String,
    pub id: String,
    pub needs_idle: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// One option of a command.
pub struct Choice {
    pub description: String,
    pub key: String,
    pub label: String,
    pub needs_idle: bool,
}

impl Choice {
    pub const fn needing_idle(mut self) -> Self {
        self.needs_idle = true;
        self
    }

    pub fn new(key: impl Into<String>, label: impl Into<String>, description: impl Into<String>) -> Self {
        Self { key: key.into(), label: label.into(), description: description.into(), needs_idle: false }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// The options of a command.
pub struct Choices {
    pub command: String,
    pub empty_message: String,
    pub items: Vec<Choice>,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
/// What a command did.
pub enum Ran {
    Choices(Choices),
    Done,
    Insert(String),
    Quit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
/// Why a command was refused.
pub enum Refused {
    Busy,
    LastSession,
    UnknownChoice,
    UnknownCommand,
}

impl From<Refusal> for Refused {
    fn from(refusal: Refusal) -> Self {
        match refusal {
            Refusal::Busy => Self::Busy,
        }
    }
}

/// Applies a choice.
type Apply = Box<dyn FnOnce(&mut Client) -> Result<(), Refused>>;

/// A choice and how to apply it.
type Pick = (Choice, Apply);

/// Runs a command.
type Run = Box<dyn Fn(&mut Client) -> Result<Ran, Refused>>;

/// What a command does.
enum Action {
    Choose(Menu),
    Run(Run),
}

/// A command with options.
struct Menu {
    empty_message: &'static str,
    picks: fn(&Client) -> Vec<Pick>,
    title: &'static str,
}

/// A command in the list.
struct Entry {
    action: Action,
    info: Command,
}

impl Entry {
    fn choose(id: &str, description: &str, menu: Menu) -> Self {
        Self::new(id, description, Action::Choose(menu))
    }

    const fn needing_idle(mut self) -> Self {
        self.info.needs_idle = true;
        self
    }

    fn new(id: &str, description: &str, action: Action) -> Self {
        Self { info: Command { id: id.to_owned(), description: description.to_owned(), needs_idle: false }, action }
    }

    fn run(id: &str, description: &str, run: impl Fn(&mut Client) -> Result<Ran, Refused> + 'static) -> Self {
        Self::new(id, description, Action::Run(Box::new(run)))
    }
}

/// The slash commands, with the Session's current choices.
fn entries(session: &Client) -> Vec<Entry> {
    let mut all = vec![
        Entry::choose(
            "model",
            "Switch the model",
            Menu { title: "Models", empty_message: "no models yet; log in, then run /refresh-models", picks: models },
        )
        .needing_idle(),
        Entry::choose(
            "thinking",
            "Set how hard the model thinks",
            Menu { title: "Thinking", empty_message: "nothing to choose", picks: levels },
        ),
        Entry::run("refresh-models", "Fetch the models each provider offers", |client| {
            client.refresh_models();
            Ok(Ran::Done)
        }),
        Entry::run("clear", "Clear the conversation", |client| {
            client.session_mut().clear()?;
            Ok(Ran::Done)
        })
        .needing_idle(),
        Entry::choose(
            "login",
            "Log in to a provider",
            Menu { title: "Log in", empty_message: "nothing to choose", picks: logins },
        ),
        Entry::choose(
            "logout",
            "Remove a saved login",
            Menu { title: "Log out", empty_message: "no saved logins", picks: logouts },
        ),
        Entry::choose(
            "persona",
            "Start a session playing a Persona",
            Menu { title: "Personas", empty_message: "no personas registered", picks: personas },
        ),
        Entry::choose(
            "session",
            "Switch to another session",
            Menu { title: "Sessions", empty_message: "no sessions", picks: sessions },
        ),
        Entry::choose(
            "close-session",
            "Close a session",
            Menu { title: "Close session", empty_message: "no sessions", picks: closable },
        ),
        Entry::run("compact", "Summarize older turns to save tokens", |client| {
            client.session_mut().compact(None)?;
            Ok(Ran::Done)
        })
        .needing_idle(),
        Entry::run("quit", "Exit", |_| Ok(Ran::Quit)),
    ];
    all.extend(session.session().skills().into_iter().map(|skill| {
        let reference = format!("{SKILL_PREFIX}{} ", skill.name);
        let id = format!("skill:{}", skill.name);
        Entry::run(&id, &skill.description, move |_| Ok(Ran::Insert(reference.clone())))
    }));
    all
}

pub fn list(session: &Client) -> Vec<Command> {
    entries(session).into_iter().map(|e| e.info).collect()
}

/// Runs a command, with a choice for commands that have options.
pub fn run(session: &mut Client, command: &str, choice: Option<&str>) -> Result<Ran, Refused> {
    let found = entries(session).into_iter().find(|e| e.info.id == command).ok_or(Refused::UnknownCommand)?;
    let busy = session.view().busy;
    if busy && found.info.needs_idle {
        return Err(Refused::Busy);
    }
    match (found.action, choice) {
        (Action::Run(run), None) => run(session),
        (Action::Run(_), Some(_)) => Err(Refused::UnknownChoice),
        (Action::Choose(menu), None) => Ok(Ran::Choices(Choices {
            command: command.to_owned(),
            title: menu.title.to_owned(),
            items: (menu.picks)(session).into_iter().map(|(c, _)| c).collect(),
            empty_message: menu.empty_message.to_owned(),
        })),
        (Action::Choose(menu), Some(key)) => {
            let (chosen, apply) =
                (menu.picks)(session).into_iter().find(|entry| entry.0.key == key).ok_or(Refused::UnknownChoice)?;
            if busy && chosen.needs_idle {
                return Err(Refused::Busy);
            }
            apply(session)?;
            Ok(Ran::Done)
        },
    }
}

/// The models to pick from, marking the current one.
fn models(session: &Client) -> Vec<Pick> {
    let current = session.view().settings.model.clone();
    session
        .models()
        .into_iter()
        .map(|(spec, name)| {
            let note = if Some(&spec) == current.as_ref() {
                "current".to_owned()
            } else if name != spec.id {
                name
            } else {
                spec.provider.display_name().to_owned()
            };
            let choice = Choice::new(spec.key(), spec.label(), note).needing_idle();
            let apply: Apply = Box::new(move |s| Ok(s.session_mut().set_model(spec)?));
            (choice, apply)
        })
        .collect()
}

fn personas(client: &Client) -> Vec<Pick> {
    client
        .list_personas()
        .into_iter()
        .map(|persona| {
            let choice = Choice::new(persona.key.clone(), persona.name, persona.description);
            let apply: Apply = Box::new(move |c| {
                let id = c.create_session(&persona.key).ok_or(Refused::UnknownChoice)?;
                let _switched = c.switch(id);
                Ok(())
            });
            (choice, apply)
        })
        .collect()
}

fn session_choices(client: &Client) -> Vec<Choice> {
    client
        .sessions()
        .into_iter()
        .map(|(id, busy)| {
            let note = match (id == client.session_id(), busy) {
                (true, true) => "current, working",
                (true, false) => "current",
                (false, true) => "working",
                (false, false) => "",
            };
            Choice::new(id.to_string(), format!("session {id}"), note)
        })
        .collect()
}

fn sessions(client: &Client) -> Vec<Pick> {
    session_choices(client)
        .into_iter()
        .map(|choice| {
            let id: u64 = choice.key.parse().unwrap_or_default();
            let apply: Apply = Box::new(move |c| if c.switch(id) { Ok(()) } else { Err(Refused::UnknownChoice) });
            (choice, apply)
        })
        .collect()
}

fn closable(client: &Client) -> Vec<Pick> {
    session_choices(client)
        .into_iter()
        .map(|choice| {
            let id: u64 = choice.key.parse().unwrap_or_default();
            let apply: Apply = Box::new(move |c| if c.close(id) { Ok(()) } else { Err(Refused::LastSession) });
            (choice, apply)
        })
        .collect()
}

/// The thinking levels to pick from, marking the current one.
fn levels(session: &Client) -> Vec<Pick> {
    let current = session.view().settings.thinking;
    Thinking::ALL
        .into_iter()
        .map(|level| {
            let note = if level == current {
                format!("current: {}", level.description())
            } else {
                level.description().to_owned()
            };
            let apply: Apply = Box::new(move |s| {
                s.session_mut().set_thinking(level);
                Ok(())
            });
            (Choice::new(level.label(), level.label(), note), apply)
        })
        .collect()
}

/// The providers to log in to, with their state.
fn logins(session: &Client) -> Vec<Pick> {
    Provider::LOGINS
        .into_iter()
        .map(|provider| {
            let note = match session.login_state(provider) {
                LoginState::Saved => "logged in".to_owned(),
                LoginState::FromEnvironment => {
                    format!("using {}", provider.env_var().unwrap_or("the environment"))
                },
                LoginState::NotLoggedIn => String::new(),
            };
            let choice = Choice::new(provider.id(), provider.display_name(), note);
            let apply: Apply = Box::new(move |s| {
                s.login(provider);
                Ok(())
            });
            (choice, apply)
        })
        .collect()
}

fn logouts(session: &Client) -> Vec<Pick> {
    session
        .saved_logins()
        .into_iter()
        .map(|provider| {
            let choice = Choice::new(provider.id(), provider.display_name(), "remove the saved login");
            let apply: Apply = Box::new(move |s| {
                s.logout(provider);
                Ok(())
            });
            (choice, apply)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::rc::Rc;
    use thoth_core::agent::controller::Reply;
    use thoth_core::agent::controller::scripted::Controller as ScriptedAgent;
    use thoth_core::harness::Event as HarnessEvent;
    use thoth_core::harness::Harness;
    use thoth_core::models::testing::fixture;
    use thoth_core::test::assert_empty;
    use thoth_core::test::scripted::Scripted;
    use thoth_core::transcript::Content;

    use thoth_core::persona::Persona;
    use thoth_core::transcript::Speaker;

    struct Fixture {
        agent: Rc<ScriptedAgent>,
        harness: Client,
    }

    fn fixture_with(agent: Rc<ScriptedAgent>, logged_in: &[Provider]) -> Fixture {
        let scripted = Scripted::new(agent, logged_in);
        let mut services = scripted.services(fixture());
        services.personas.add(Persona::new("helper", "Helper", "helps")).unwrap();
        let mut harness = Harness::new(services);
        harness.start_session_with(1, Box::new(Rc::clone(&scripted.agent)));
        Fixture { harness: Client::open(harness).unwrap(), agent: scripted.agent }
    }

    fn fresh() -> Fixture {
        fixture_with(ScriptedAgent::new(), &[])
    }

    fn labels(choices: &Choices) -> Vec<String> {
        choices.items.iter().map(|c| c.label.clone()).collect()
    }

    fn open(session: &mut Client, name: &str) -> Choices {
        match run(session, name, None) {
            Ok(Ran::Choices(choices)) => choices,
            other => panic!("{name} should list its choices, got {other:?}"),
        }
    }

    fn choose(session: &mut Client, choices: &Choices, label: &str) -> Result<Ran, Refused> {
        let choice = choices.items.iter().find(|c| c.label.contains(label)).ok_or(Refused::UnknownChoice)?;
        run(session, &choices.command, Some(&choice.key))
    }

    #[test]
    fn the_commands_are_the_built_ins_and_the_skills() {
        let f = fixture_with(ScriptedAgent::new().with_skill("tdd", "Test first"), &[]);
        let ids: Vec<_> = list(&f.harness).into_iter().map(|c| c.id).collect();
        assert_eq!(
            ids,
            [
                "model",
                "thinking",
                "refresh-models",
                "clear",
                "login",
                "logout",
                "persona",
                "session",
                "close-session",
                "compact",
                "quit",
                "skill:tdd"
            ]
        );
    }

    #[test]
    fn model_and_clear_wait_for_an_idle_agent() {
        let idle: Vec<_> = list(&fresh().harness).into_iter().filter(|c| c.needs_idle).map(|c| c.id).collect();
        assert_eq!(idle, ["model", "clear", "compact"]);
    }

    #[test]
    fn choosing_an_agent_starts_a_session_and_shows_it() {
        let mut f = fresh();
        let agents = open(&mut f.harness, "persona");
        assert_eq!(labels(&agents), ["Helper"]);
        assert_eq!(choose(&mut f.harness, &agents, "Helper"), Ok(Ran::Done));
        assert_eq!(f.harness.sessions().len(), 2);
        assert_ne!(f.harness.session_id(), 1);
        assert_eq!(f.harness.view().settings.model, None, "a new agent of its own");
    }

    #[test]
    fn sessions_are_listed_and_switched_between() {
        let mut f = fresh();
        let agents = open(&mut f.harness, "persona");
        choose(&mut f.harness, &agents, "Helper").unwrap();
        let sessions = open(&mut f.harness, "session");
        assert_eq!(labels(&sessions), ["session 1", "session 2"]);
        assert_eq!(choose(&mut f.harness, &sessions, "session 1"), Ok(Ran::Done));
        assert_eq!(f.harness.session_id(), 1);
        assert_eq!(run(&mut f.harness, "session", Some("99")), Err(Refused::UnknownChoice));
    }

    #[test]
    fn closing_a_session_moves_to_another_and_the_last_one_stays() {
        let mut f = fresh();
        let agents = open(&mut f.harness, "persona");
        choose(&mut f.harness, &agents, "Helper").unwrap();
        assert_eq!(f.harness.session_id(), 2);
        assert_eq!(run(&mut f.harness, "close-session", Some("2")), Ok(Ran::Done));
        assert_eq!(f.harness.session_id(), 1);
        assert_eq!(f.harness.sessions().len(), 1);
        assert_eq!(run(&mut f.harness, "close-session", Some("1")), Err(Refused::LastSession));
    }

    #[test]
    fn thinking_lists_the_levels_marks_the_current_one_and_sets_the_chosen_one() {
        let mut f = fresh();
        let levels = open(&mut f.harness, "thinking");
        assert_eq!(levels.title, "Thinking");
        assert_eq!(labels(&levels), ["off", "minimal", "low", "medium", "high", "max"]);
        let current = f.harness.view().settings.thinking;
        let marked = levels.items.iter().filter(|c| c.description.starts_with("current")).count();
        assert_eq!(marked, 1);
        assert!(levels.items.iter().any(|c| c.key == current.label() && c.description.starts_with("current")));

        assert_eq!(choose(&mut f.harness, &levels, "high"), Ok(Ran::Done));
        assert_eq!(f.harness.view().settings.thinking, Thinking::High);
    }

    #[test]
    fn model_lists_the_models_marks_the_current_one_and_switches_to_the_chosen_one() {
        let mut f = fresh();
        let models = open(&mut f.harness, "model");
        assert_eq!(models.title, "Models");
        assert_eq!(models.items.len(), 8);
        assert_eq!(models.items[0].label, "bonsai-2:2b (ollama/openai)");
        assert_eq!(models.items[0].description, "current");
        assert_eq!(models.items[1].description, "Ollama");
        assert!(models.items.iter().all(|m| m.needs_idle));

        assert_eq!(choose(&mut f.harness, &models, "llama3"), Ok(Ran::Done));
        assert_eq!(f.agent.model().map(|m| m.id), Some("llama3.2:3b".to_owned()));
    }

    #[test]
    fn a_choice_is_run_by_its_key_without_asking_for_the_list_first() {
        let mut f = fresh();
        assert_eq!(run(&mut f.harness, "model", Some("openai/openai/gpt-4o")), Ok(Ran::Done));
        assert_eq!(f.agent.model().map(|m| m.id), Some("gpt-4o".to_owned()));
    }

    #[test]
    fn clear_empties_the_transcript_and_the_conversation() {
        let mut f = fresh();
        f.harness.submit("hello".into(), "hello".into());
        f.agent.reply(Reply::Done { usage: None });
        f.harness.session_mut().poll();
        assert_eq!(run(&mut f.harness, "clear", None), Ok(Ran::Done));
        assert!(
            f.harness
                .view()
                .transcript
                .items()
                .iter()
                .all(|i| !matches!(&i.content, Content::Message(m) if m.speaker == Speaker::You))
        );
        assert_eq!(f.agent.cleared(), 1);
    }

    #[test]
    fn model_and_clear_are_refused_while_a_turn_runs() {
        let mut f = fresh();
        let models = open(&mut f.harness, "model");
        f.harness.submit("hello".into(), "hello".into());
        assert_eq!(run(&mut f.harness, "clear", None), Err(Refused::Busy));
        assert_eq!(run(&mut f.harness, "model", None), Err(Refused::Busy));
        assert_eq!(f.agent.cleared(), 0);
        assert_eq!(choose(&mut f.harness, &models, "llama3"), Err(Refused::Busy));
    }

    #[test]
    fn quit_asks_the_terminal_to_quit() {
        assert_eq!(run(&mut fresh().harness, "quit", None), Ok(Ran::Quit));
    }

    #[test]
    fn a_skill_command_inserts_its_reference() {
        let mut f = fixture_with(ScriptedAgent::new().with_skill("tdd", "Test first"), &[]);
        assert_eq!(run(&mut f.harness, "skill:tdd", None), Ok(Ran::Insert("/skill:tdd ".into())));
    }

    #[test]
    fn refresh_models_starts_a_refresh() {
        let mut f = fresh();
        assert_eq!(run(&mut f.harness, "refresh-models", None), Ok(Ran::Done));
        let notices = f.harness.poll();
        assert!(notices.iter().any(|e| matches!(e, HarnessEvent::Notice { speaker: Speaker::Models, .. })));
    }

    #[test]
    fn an_unknown_command_or_choice_is_refused_not_ignored() {
        let mut f = fresh();
        assert_eq!(run(&mut f.harness, "nope", None), Err(Refused::UnknownCommand));
        assert_eq!(run(&mut f.harness, "model", Some("nowhere/openai/none")), Err(Refused::UnknownChoice));
        assert_eq!(run(&mut f.harness, "quit", Some("now")), Err(Refused::UnknownChoice));
    }

    #[test]
    fn login_and_logout_list_their_own_providers() {
        let mut f = fixture_with(ScriptedAgent::new(), &[Provider::OpenAi]);
        let logins = open(&mut f.harness, "login");
        assert_eq!(labels(&logins), ["GitHub Copilot", "OpenAI", "Anthropic"]);
        assert_eq!(logins.items[1].description, "logged in");
        assert_eq!(labels(&open(&mut f.harness, "logout")), ["OpenAI"]);
    }

    #[test]
    fn choosing_a_provider_starts_its_login() {
        let mut f = fresh();
        let logins = open(&mut f.harness, "login");
        assert_eq!(choose(&mut f.harness, &logins, "Copilot"), Ok(Ran::Done));
        assert!(f.harness.login_view().active);
    }

    #[test]
    fn logout_removes_the_saved_login_and_the_old_list_is_then_stale() {
        let mut f = fixture_with(ScriptedAgent::new(), &[Provider::Copilot]);
        let saved = open(&mut f.harness, "logout");
        assert_eq!(choose(&mut f.harness, &saved, "GitHub Copilot"), Ok(Ran::Done));
        assert_eq!(choose(&mut f.harness, &saved, "GitHub Copilot"), Err(Refused::UnknownChoice));
        let empty = open(&mut f.harness, "logout");
        assert_empty(&empty.items);
        assert_eq!(empty.empty_message, "no saved logins");
    }
}
