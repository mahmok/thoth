use core::future::Future;
use std::collections::HashMap;
use std::sync::Mutex;

use crate::llm::thinking::Thinking;
use crate::sync::MutexExt as _;

use super::ClientError;

/// Words in an error that point at a request setting.
const SETTING_WORDS: [&str; 6] = ["reason", "effort", "thinking", "budget", "summary", "include"];

#[derive(Default, Debug)]
/// Remembers which setting a model accepts.
pub(super) struct Negotiation {
    worked: Mutex<HashMap<Thinking, usize>>,
}

impl Negotiation {
    #[inline]
    /// Sends with the setting that last worked, falling back to the next when the model refuses one.
    ///
    /// # Errors
    ///
    /// Fails with the last error when no setting is accepted, or at once with an error that is not about a setting.
    pub(super) async fn send<Callback, S, T, Fut>(
        &self,
        level: Thinking,
        settings: &[S],
        send: Callback,
    ) -> Result<T, ClientError>
    where
        S: Sync,
        Fut: Future<Output = Result<T, ClientError>> + Send,
        Callback: Fn(&S) -> Fut + Send,
    {
        let start = self.worked.lock_unpoisoned().get(&level).copied().unwrap_or(0);
        let mut next = start;
        loop {
            let Some(setting) = settings.get(next) else {
                return Err(ClientError::Api("there are no request settings to try".into()));
            };
            match send(setting).await {
                Ok(done) => {
                    let _previous = self.worked.lock_unpoisoned().insert(level, next);
                    return Ok(done);
                },
                Err(error) if is_setting_refusal(&error) && next.saturating_add(1) < settings.len() => {
                    next = next.saturating_add(1);
                },
                Err(error) => return Err(error),
            }
        }
    }
}

fn is_setting_refusal(error: &ClientError) -> bool {
    error.bad_request().is_some_and(|message| {
        let lowered = message.to_lowercase();
        message.is_empty() || SETTING_WORDS.iter().any(|word| lowered.contains(word))
    })
}

#[cfg(test)]
mod tests {
    use core::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    fn rejected() -> ClientError {
        ClientError::BadRequest("unknown field reasoning_effort".into())
    }

    async fn attempt(fallback: &Negotiation, accepts: usize, calls: &AtomicUsize) -> Result<usize, ClientError> {
        fallback
            .send(Thinking::High, &[0, 1, 2], |&setting| {
                calls.fetch_add(1, Ordering::Relaxed);
                async move { if setting >= accepts { Ok(setting) } else { Err(rejected()) } }
            })
            .await
    }

    #[tokio::test]
    async fn no_settings_is_an_error_and_not_a_panic() {
        let none: [usize; 0] = [];
        let result = Negotiation::default().send(Thinking::High, &none, |_| async { Ok(0usize) }).await;
        assert!(matches!(result, Err(ClientError::Api(_))));
    }

    #[tokio::test]
    async fn a_rejected_setting_falls_back_to_the_next() {
        let calls = AtomicUsize::new(0);
        assert_eq!(attempt(&Negotiation::default(), 2, &calls).await.unwrap(), 2);
        assert_eq!(calls.load(Ordering::Relaxed), 3);
    }

    #[tokio::test]
    async fn the_setting_that_worked_is_tried_first_next_time() {
        let fallback = Negotiation::default();
        let calls = AtomicUsize::new(0);
        attempt(&fallback, 1, &calls).await.unwrap();
        calls.store(0, Ordering::Relaxed);
        assert_eq!(attempt(&fallback, 1, &calls).await.unwrap(), 1);
        assert_eq!(calls.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn a_bad_request_about_something_else_is_not_retried() {
        let calls = AtomicUsize::new(0);
        let result: Result<(), ClientError> = Negotiation::default()
            .send(Thinking::Low, &[0i32, 1i32, 2i32], |_| {
                calls.fetch_add(1, Ordering::Relaxed);
                async { Err(ClientError::BadRequest("tools[0].parameters is invalid".into())) }
            })
            .await;
        assert!(result.is_err());
        assert_eq!(calls.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn a_refusal_with_no_explanation_is_retried() {
        let calls = AtomicUsize::new(0);
        let _unused: Result<(), ClientError> = Negotiation::default()
            .send(Thinking::Low, &[0i32, 1i32], |_| {
                calls.fetch_add(1, Ordering::Relaxed);
                async { Err(ClientError::BadRequest(String::new())) }
            })
            .await;
        assert_eq!(calls.load(Ordering::Relaxed), 2);
    }

    #[tokio::test]
    async fn the_last_failure_is_reported_and_other_errors_are_not_retried() {
        let calls = AtomicUsize::new(0);
        attempt(&Negotiation::default(), 9, &calls).await.unwrap_err();
        assert_eq!(calls.load(Ordering::Relaxed), 3);

        let failing_calls = AtomicUsize::new(0);
        let result: Result<(), ClientError> = Negotiation::default()
            .send(Thinking::Low, &[0i32, 1i32], |_| {
                failing_calls.fetch_add(1, Ordering::Relaxed);
                async { Err(ClientError::Api("boom".into())) }
            })
            .await;
        assert!(result.is_err());
        assert_eq!(failing_calls.load(Ordering::Relaxed), 1);
    }
}
