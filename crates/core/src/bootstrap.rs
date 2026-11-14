use alloc::sync::Arc;
use std::env;
use std::path::PathBuf;

use crate::agent::context::Context as AgentContext;
use crate::agent::skills::SkillSet;
use crate::agent_builder::AgentBuilder;
use crate::auth::session::Login as LoginSession;
use crate::auth::store::Credentials as CredentialStore;
use crate::auth::worker::TokioLogin as TokioLoginWorker;
use crate::builtin::files::FileReferences;
use crate::harness::Harness;
use crate::models::fetch::ModelFetcher;
use crate::models::refresh::Session as RefreshSession;
use crate::models::store::{InMemory as InMemoryModelsStore, Json as JsonModelsStore, Models as ModelsStore};
use crate::models::worker::TokioRefresh as TokioRefreshWorker;
use crate::persona::Repository as PersonaRepository;
use crate::services::Services;
use crate::workspace::Workspace;
use anyhow::Context as _;

#[inline]
/// Builds the Harness: credentials, models, skills, Personas and the file index.
///
/// # Errors
///
/// Fails if the Workspace, skills, AGENTS.md files or Personas cannot be loaded.
pub async fn start() -> anyhow::Result<Harness> {
    let (store, store_notice) = open_credentials();
    let (models, models_notice) = open_models();
    let notice: Vec<String> = [store_notice, models_notice].into_iter().flatten().collect();

    let workspace = Workspace::from_current_dir()?;
    let skills = load_skills(&workspace)?;
    let context = AgentContext::new(workspace.root()).with_agent_files().context("Failed to load AGENTS.md files")?;
    let builder = AgentBuilder::new(Arc::clone(&store), Arc::clone(&models), workspace.clone(), context, skills);
    let mut personas = PersonaRepository::default();

    personas.load().await.context("Failed to load personas")?;
    let files = Arc::new(FileReferences::new(workspace.clone()));

    let harness = Harness::new(Services {
        personas,
        builder,
        logins: LoginSession::new(Box::new(TokioLoginWorker::new(Arc::clone(&store))), Arc::clone(&store)),
        refresh: RefreshSession::new(Box::new(TokioRefreshWorker::new(
            ModelFetcher::new(Arc::clone(&store)),
            Arc::clone(&models),
        ))),
        credentials: store,
        models,
        files,
    });
    if notice.is_empty() { Ok(harness) } else { Ok(harness.with_notice(notice.join("\n"))) }
}

fn open_credentials() -> (Arc<CredentialStore>, Option<String>) {
    let Some(path) = CredentialStore::default_path() else {
        let notice = "No config directory found. Logins will not be saved.";
        return (Arc::new(CredentialStore::unsaved()), Some(notice.into()));
    };
    match CredentialStore::open(&path) {
        Ok(store) => (Arc::new(store), None),
        Err(e) => {
            let notice = format!("Could not read {}: {e}. Logins will not be saved this session.", path.display());
            (Arc::new(CredentialStore::unsaved()), Some(notice))
        },
    }
}

fn open_models() -> (Arc<dyn ModelsStore>, Option<String>) {
    JsonModelsStore::default_path().map_or_else(
        || -> (Arc<dyn ModelsStore>, Option<String>) { (Arc::new(InMemoryModelsStore::default()), None) },
        |path| -> (Arc<dyn ModelsStore>, Option<String>) {
            let (store, warning) = JsonModelsStore::open(path);
            (Arc::new(store), warning)
        },
    )
}

fn load_skills(workspace: &Workspace) -> anyhow::Result<SkillSet> {
    let home = env::var_os("HOME").map(PathBuf::from);
    let mut roots = vec![workspace.root()];
    roots.extend(home.as_deref());
    SkillSet::from_default_dirs(&roots).context("Failed to load skills")
}
