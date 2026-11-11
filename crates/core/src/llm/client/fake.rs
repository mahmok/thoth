use alloc::collections::VecDeque;
use alloc::sync::Arc;
use std::sync::Mutex;
use tokio::task;

use super::{ClientError, CreateCompletionRequest, Delta, Llm as LlmClient, StreamedCompletion, ToolChoice, ToolSpec};
use crate::sync::MutexExt as _;

/// The replies a fake client gives.
type Script = VecDeque<Result<StreamedCompletion, ClientError>>;
/// The requests a fake client saw: how many messages, and which Tools.
pub type Requests = Vec<(usize, Vec<ToolSpec>)>;

#[derive(Default, Debug)]
/// The shared state of a fake client.
struct Inner {
    choices: Mutex<Vec<ToolChoice>>,
    requests: Mutex<Requests>,
    script: Mutex<Script>,
}

#[derive(Clone, Default, Debug)]
/// A client that replays a script.
pub struct Client {
    inner: Arc<Inner>,
}

impl Client {
    #[must_use]
    #[inline]
    /// Creates a client that replies with `script`, in order.
    pub fn new(script: Vec<Result<StreamedCompletion, ClientError>>) -> Self {
        let client = Self::default();
        *client.inner.script.lock_unpoisoned() = script.into();
        client
    }

    #[must_use]
    #[inline]
    pub fn requests(&self) -> Requests {
        self.inner.requests.lock_unpoisoned().clone()
    }

    /// The tool choice of each request, in order.
    #[must_use]
    #[inline]
    pub fn tool_choices(&self) -> Vec<ToolChoice> {
        self.inner.choices.lock_unpoisoned().clone()
    }
}

impl LlmClient for Client {
    #[inline]
    async fn create_completion_stream<F>(
        &self,
        request: CreateCompletionRequest<'_>,
        mut on_delta: F,
    ) -> Result<StreamedCompletion, ClientError>
    where
        F: FnMut(Delta<'_>) + Send,
    {
        self.inner.requests.lock_unpoisoned().push((request.messages.len(), request.tools.to_vec()));
        self.inner.choices.lock_unpoisoned().push(request.tool_choice);
        task::yield_now().await;
        let next = self.inner.script.lock_unpoisoned().pop_front().expect("script exhausted")?;
        if !next.text.is_empty() {
            on_delta(Delta::Text(&next.text));
        }
        Ok(next)
    }
}

#[must_use]
#[inline]
pub fn failure() -> ClientError {
    ClientError::Api("fake failure".into())
}
