use tokio::sync::mpsc::UnboundedReceiver;

use crate::llm::provider::{LoginMethod, Provider};

use super::Failure as AuthError;
use super::credential::Credential;
use super::github::Github;
use super::store::Credentials as CredentialStore;

#[derive(Debug, Clone, PartialEq, Eq)]
/// Something a login reports.
pub enum LoginEvent {
    /// GitHub issued a code to enter.
    DeviceCode {
        /// Where to enter the code.
        url: String,
        /// The code to enter.
        code: String,
    },
    /// The login succeeded.
    Done(Provider),
    /// The login failed.
    Failed(String),
    /// A key is needed from the user.
    PromptSecret {
        /// What to ask the user.
        message: String,
    },
}

#[inline]
/// Logs in to `provider` and reports progress through `emit`.
pub async fn run<Callback>(
    provider: Provider,
    github: &Github,
    store: &CredentialStore,
    answers: &mut UnboundedReceiver<String>,
    emit: Callback,
) where
    Callback: Fn(LoginEvent) + Sync,
{
    let result = match provider.login_method() {
        LoginMethod::GithubDevice => login_copilot(github, store, &emit).await,
        LoginMethod::ApiKey => login_api_key(provider, store, answers, &emit).await,
        LoginMethod::None => Err(AuthError::Flow(format!("{} needs no login", provider.display_name()))),
    };
    emit(match result {
        Ok(()) => LoginEvent::Done(provider),
        Err(error) => LoginEvent::Failed(error.to_string()),
    });
}

async fn login_copilot(
    github: &Github,
    store: &CredentialStore,
    emit: &(impl Fn(LoginEvent) + Sync),
) -> Result<(), AuthError> {
    let device = github.start_device_flow().await?;
    emit(LoginEvent::DeviceCode { url: device.verification_uri.clone(), code: device.user_code.clone() });

    let github_token = github.poll_token(&device).await?;
    let credential = github.exchange_token(&github_token).await?;
    store.write(Provider::Copilot, credential).map_err(|e| AuthError::Store(e.to_string()))
}

/// Asks for an API key and saves it.
async fn login_api_key(
    provider: Provider,
    store: &CredentialStore,
    answers: &mut UnboundedReceiver<String>,
    emit: &(impl Fn(LoginEvent) + Sync),
) -> Result<(), AuthError> {
    emit(LoginEvent::PromptSecret {
        message: format!("Paste your {} API key (input is hidden)", provider.display_name()),
    });
    let answer = answers.recv().await.ok_or_else(|| AuthError::Flow("login cancelled".into()))?;
    let key = answer.trim();
    if key.is_empty() {
        return Err(AuthError::Flow("no key entered".into()));
    }
    store.write(provider, Credential::ApiKey { key: key.to_owned() }).map_err(|e| AuthError::Store(e.to_string()))
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use tokio::sync::mpsc;

    use super::*;
    use crate::auth::github::Endpoints;
    use crate::auth::testing::MockServer;

    struct Outcome {
        events: Vec<LoginEvent>,
        store: CredentialStore,
    }

    async fn login(provider: Provider, server: Option<&MockServer>, answers: &[&str]) -> Outcome {
        let store = CredentialStore::in_memory();
        let (tx, mut rx) = mpsc::unbounded_channel();
        for answer in answers {
            tx.send(answer.to_string()).unwrap();
        }
        drop(tx);
        let github = Github::new(server.map_or_else(Endpoints::github, MockServer::endpoints));
        let events = Mutex::new(Vec::new());
        run(provider, &github, &store, &mut rx, |event| events.lock().unwrap().push(event)).await;
        Outcome { events: events.into_inner().unwrap(), store }
    }

    fn copilot_server() -> impl Future<Output = MockServer> {
        MockServer::start(vec![
            (
                "/login/device/code",
                vec![(200, r#"{"device_code":"d","user_code":"WXYZ-9999","verification_uri":"https://github.com/login/device","interval":0,"expires_in":60}"#.into())],
            ),
            ("/login/oauth/access_token", vec![(200, r#"{"access_token":"gho_1"}"#.into())]),
            ("/copilot_internal/v2/token", vec![(200, r#"{"token":"copilot-tok","expires_at":4000000000}"#.into())]),
        ])
    }

    #[tokio::test]
    async fn copilot_login_shows_the_code_and_stores_the_credential() {
        let server = copilot_server().await;
        let outcome = login(Provider::Copilot, Some(&server), &[]).await;

        assert_eq!(
            outcome.events,
            [
                LoginEvent::DeviceCode { url: "https://github.com/login/device".into(), code: "WXYZ-9999".into() },
                LoginEvent::Done(Provider::Copilot),
            ]
        );
        assert!(matches!(outcome.store.read(Provider::Copilot),
            Some(Credential::Oauth { refresh, access, .. }) if refresh == "gho_1" && access == "copilot-tok"));
        let paths: Vec<String> = server.requests().into_iter().map(|r| r.path).collect();
        assert!(paths.iter().all(|path| !path.contains("/policy")), "{paths:?}");
    }

    #[tokio::test]
    async fn a_denied_copilot_login_reports_the_failure_and_stores_nothing() {
        let server = MockServer::start(vec![
            (
                "/login/device/code",
                vec![(200, r#"{"device_code":"d","user_code":"C","verification_uri":"https://x.test","interval":0,"expires_in":60}"#.into())],
            ),
            ("/login/oauth/access_token", vec![(200, r#"{"error":"access_denied"}"#.into())]),
        ])
        .await;
        let outcome = login(Provider::Copilot, Some(&server), &[]).await;
        assert!(matches!(outcome.events.last(), Some(LoginEvent::Failed(m)) if m.contains("access_denied")));
        assert!(outcome.store.read(Provider::Copilot).is_none());
    }

    #[tokio::test]
    async fn an_api_key_is_asked_for_trimmed_and_stored() {
        let outcome = login(Provider::OpenAi, None, &["  sk-test \n"]).await;
        assert!(matches!(&outcome.events[0], LoginEvent::PromptSecret { message } if message.contains("OpenAI")));
        assert_eq!(outcome.events[1], LoginEvent::Done(Provider::OpenAi));
        assert_eq!(outcome.store.read(Provider::OpenAi), Some(Credential::ApiKey { key: "sk-test".into() }));
    }

    #[tokio::test]
    async fn an_empty_key_is_refused() {
        let outcome = login(Provider::Anthropic, None, &["   "]).await;
        assert!(matches!(outcome.events.last(), Some(LoginEvent::Failed(m)) if m == "no key entered"));
        assert!(outcome.store.read(Provider::Anthropic).is_none());
    }

    #[tokio::test]
    async fn closing_the_prompt_cancels_the_login() {
        let outcome = login(Provider::Anthropic, None, &[]).await;
        assert!(matches!(outcome.events.last(), Some(LoginEvent::Failed(m)) if m == "login cancelled"));
    }

    #[tokio::test]
    async fn ollama_has_nothing_to_log_in_to() {
        let outcome = login(Provider::Ollama, None, &[]).await;
        assert!(matches!(outcome.events.last(), Some(LoginEvent::Failed(_))));
    }
}
