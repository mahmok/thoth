use alloc::collections::VecDeque;
use alloc::sync::Arc;
use core::time::Duration;
use std::sync::Mutex;
use tokio::task;
use tokio::time;

use crate::llm::client::chat::{FunctionCall, ToolCall, ToolCallKind};
use crate::llm::client::error::Failure as ClientError;
use crate::llm::client::{CreateCompletionRequest, Delta, Llm as LlmClient, StreamedCompletion, ToolChoice, ToolSpec};
use crate::sync::MutexExt as _;

#[derive(Debug)]
/// One thing the scripted client does when it is asked for a reply.
pub enum Step {
    /// Answer with `text`.
    Reply(String),
    /// Call one Tool.
    ToolCall { id: String, name: String, arguments: String },
    /// Call several Tools in one reply.
    ToolCalls(Vec<(String, String, String)>),
    /// Answer with a whole completion: reasoning, usage, a cut-off, and so on.
    Completion(Box<StreamedCompletion>),
    /// Wait before doing whatever comes next.
    Delay(Duration),
    /// Fail the request.
    Error(ClientError),
}

/// The steps a scripted client replays, in order.
pub type Script = Vec<Step>;

#[must_use]
#[inline]
/// A reply that answers with `text`.
pub fn reply<IntoString>(text: IntoString) -> Step
where
    IntoString: Into<String>,
{
    Step::Reply(text.into())
}

#[must_use]
#[inline]
/// A reply that calls `name` once, with `arguments` as its raw JSON.
pub fn call(id: &str, name: &str, arguments: &str) -> Step {
    Step::ToolCall { id: id.into(), name: name.into(), arguments: arguments.into() }
}

#[must_use]
#[inline]
/// A reply that calls `name` once for each `(call id, arguments)`.
pub fn tool_calls(name: &str, entries: &[(&str, &str)]) -> Step {
    calls(&entries.iter().map(|&(id, arguments)| (id, name, arguments)).collect::<Vec<_>>())
}

#[must_use]
#[inline]
/// A reply that calls each `(call id, name, arguments)` once.
pub fn calls(entries: &[(&str, &str, &str)]) -> Step {
    Step::ToolCalls(entries.iter().map(|&(id, name, arguments)| (id.into(), name.into(), arguments.into())).collect())
}

#[must_use]
#[inline]
/// A reply with a whole completion, for the parts a plain reply cannot say: reasoning, usage,
/// a cut-off.
pub fn completion(completion: StreamedCompletion) -> Step {
    Step::Completion(Box::new(completion))
}

#[must_use]
#[inline]
/// Waits `duration` before doing whatever the script says next.
pub const fn delay(duration: Duration) -> Step {
    Step::Delay(duration)
}

#[must_use]
#[inline]
/// The error the scripted client gives when it fails.
pub fn failure() -> Step {
    Step::Error(ClientError::Api("fake failure".into()))
}

#[must_use]
#[inline]
/// Fails the request with `error`.
pub const fn error(error: ClientError) -> Step {
    Step::Error(error)
}

#[must_use]
#[inline]
/// A completion that answers with `text`, for when a test needs the whole value.
pub fn answer<IntoString>(text: IntoString) -> StreamedCompletion
where
    IntoString: Into<String>,
{
    StreamedCompletion { text: text.into(), ..Default::default() }
}

#[must_use]
#[inline]
/// A completion that calls `name` once, for when a test needs the whole value.
pub fn call_answer(id: &str, name: &str, arguments: &str) -> StreamedCompletion {
    StreamedCompletion {
        tool_calls: vec![ToolCall {
            id: id.into(),
            kind: ToolCallKind::Function,
            function: FunctionCall { arguments: arguments.into(), name: name.into() },
        }],
        ..Default::default()
    }
}

#[must_use]
#[inline]
/// A completion that calls each `(call id, name, arguments)` once, for when a test needs the
/// whole value.
pub fn calls_answer(entries: &[(&str, &str, &str)]) -> StreamedCompletion {
    StreamedCompletion {
        tool_calls: entries
            .iter()
            .map(|&(id, name, arguments)| ToolCall {
                id: id.into(),
                kind: ToolCallKind::Function,
                function: FunctionCall { arguments: arguments.into(), name: name.into() },
            })
            .collect(),
        ..Default::default()
    }
}

#[derive(Default, Debug)]
/// The shared state of a scripted client.
struct Inner {
    choices: Mutex<Vec<ToolChoice>>,
    requests: Mutex<Vec<(usize, Vec<ToolSpec>)>>,
    script: Mutex<VecDeque<Step>>,
}

#[derive(Clone, Default, Debug)]
/// A client that replays a script.
pub struct Client {
    inner: Arc<Inner>,
}

impl Client {
    #[must_use]
    #[inline]
    /// Creates a client that follows `script`, in order.
    pub fn new(script: Script) -> Self {
        let client = Self::default();
        *client.inner.script.lock_unpoisoned() = script.into();
        client
    }

    #[must_use]
    #[inline]
    /// The requests the client saw: how many messages, and which Tools.
    pub fn requests(&self) -> Vec<(usize, Vec<ToolSpec>)> {
        self.inner.requests.lock_unpoisoned().clone()
    }

    /// The tool choice of each request, in order.
    #[must_use]
    #[inline]
    pub fn tool_choices(&self) -> Vec<ToolChoice> {
        self.inner.choices.lock_unpoisoned().clone()
    }

    fn next(&self) -> Step {
        self.inner.script.lock_unpoisoned().pop_front().expect("script exhausted")
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
        loop {
            match self.next() {
                Step::Delay(duration) => time::sleep(duration).await,
                Step::Error(error) => return Err(error),
                Step::Reply(text) => {
                    if !text.is_empty() {
                        on_delta(Delta::Text(&text));
                    }
                    return Ok(StreamedCompletion { text, ..Default::default() });
                },
                Step::ToolCall { id, name, arguments } => {
                    return Ok(StreamedCompletion {
                        tool_calls: vec![ToolCall {
                            id,
                            kind: ToolCallKind::Function,
                            function: FunctionCall { arguments, name },
                        }],
                        ..Default::default()
                    });
                },
                Step::ToolCalls(calls) => {
                    return Ok(StreamedCompletion {
                        tool_calls: calls
                            .into_iter()
                            .map(|(id, name, arguments)| ToolCall {
                                id,
                                kind: ToolCallKind::Function,
                                function: FunctionCall { arguments, name },
                            })
                            .collect(),
                        ..Default::default()
                    });
                },
                Step::Completion(completion) => {
                    if !completion.text.is_empty() {
                        on_delta(Delta::Text(&completion.text));
                    }
                    return Ok(*completion);
                },
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_delay_comes_before_the_step_after_it() {
        let client = Client::new(vec![delay(Duration::from_millis(10)), reply("after")]);
        let started = std::time::Instant::now();
        let completion =
            LlmClient::create_completion_stream(&client, CreateCompletionRequest::default(), |_| {}).await.unwrap();

        assert_eq!(completion.text, "after");
        assert!(started.elapsed() >= Duration::from_millis(10));
    }
}
