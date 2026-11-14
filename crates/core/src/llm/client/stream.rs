//! One driver for every streamed reply.

use core::future::Future;

use reqwest::Response;

use super::{ClientError, Delta, EventStream, StreamedCompletion};

/// What one protocol makes of a streamed event.
pub(super) trait Adapter: Sized {
    /// Applies one event, reporting reasoning and text as it arrives.
    fn apply(&mut self, data: &str, on_delta: &mut impl FnMut(Delta<'_>)) -> Result<(), ClientError>;

    /// Whether this event ends the stream before the connection does.
    fn is_end(&self, _data: &str) -> bool {
        false
    }

    /// The completion the events produced.
    fn finish(self) -> StreamedCompletion;
}

/// Runs one stream: post, iterate events, apply, finish, and fail if cut off.
pub(super) async fn drive<A, F, D>(post: D, mut state: A, mut on_delta: F) -> Result<StreamedCompletion, ClientError>
where
    A: Adapter,
    F: FnMut(Delta<'_>) + Send,
    D: Future<Output = Result<Response, ClientError>>,
{
    let response = post.await?;
    let mut events = EventStream::new(response);
    while let Some(data) = events.next().await? {
        if state.is_end(&data) {
            break;
        }
        state.apply(&data, &mut on_delta)?;
    }
    state.finish().fail_if_cut_off_empty()
}

#[cfg(test)]
mod tests {
    use alloc::sync::Arc;

    use crate::auth::Auth;
    use crate::auth::store::Credentials as CredentialStore;
    use crate::auth::testing::MockServer;
    use crate::llm::client::anthropic::{Client as AnthropicClient, Config as AnthropicConfig};
    use crate::llm::client::chat::Message;
    use crate::llm::client::openai::{OpenAiClient, OpenAiConfig};
    use crate::llm::client::responses::{Client as ResponsesClient, Config as ResponsesConfig};
    use crate::llm::client::{ClientError, CreateCompletionRequest, Delta, Llm as _, StreamedCompletion};
    use crate::llm::provider::{Protocol, Provider};

    /// What one scripted reply produced.
    #[derive(Debug)]
    struct Streamed {
        done: StreamedCompletion,
        reasoning: Vec<String>,
        text: Vec<String>,
    }

    /// Asks a real client, over HTTP, against a scripted event stream.
    async fn ask(protocol: Protocol, reply: &str) -> Result<Streamed, ClientError> {
        let path = match protocol {
            Protocol::OpenAi => "/chat/completions",
            Protocol::Anthropic => "/v1/messages",
            Protocol::Responses => "/responses",
            Protocol::Google => unreachable!("the google protocol has no client"),
        };
        let server = MockServer::start(vec![(path, vec![(200, reply.to_owned())])]).await;
        let auth = Auth::new(Provider::builtin("ollama").unwrap(), protocol, Arc::new(CredentialStore::in_memory()))
            .with_base_url(&server.url);
        let messages = [Message::user("hi")];
        let request = CreateCompletionRequest { messages: &messages, ..Default::default() };
        let mut text_deltas: Vec<String> = Vec::new();
        let mut reasoning_deltas: Vec<String> = Vec::new();
        let mut on_delta = |delta: Delta<'_>| match delta {
            Delta::Text(text) => text_deltas.push(text.to_owned()),
            Delta::Reasoning(text) => reasoning_deltas.push(text.to_owned()),
        };
        let done = match protocol {
            Protocol::OpenAi => {
                OpenAiClient::new(OpenAiConfig { auth, model: "m".into() })
                    .create_completion_stream(request, &mut on_delta)
                    .await
            },
            Protocol::Anthropic => {
                AnthropicClient::new(AnthropicConfig { auth, max_tokens: 99, model: "m".into() })
                    .create_completion_stream(request, &mut on_delta)
                    .await
            },
            Protocol::Responses => {
                ResponsesClient::new(ResponsesConfig { auth, model: "m".into() })
                    .create_completion_stream(request, &mut on_delta)
                    .await
            },
            Protocol::Google => unreachable!("the google protocol has no client"),
        };
        done.map(|done| Streamed { done, reasoning: reasoning_deltas, text: text_deltas })
    }

    #[tokio::test]
    async fn every_protocol_streams_text_and_reports_usage() {
        let replies = [
            (
                Protocol::OpenAi,
                concat!(
                    "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"he\"}}]}\n\n",
                    "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"llo\"}}]}\n\n",
                    "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":2,\"total_tokens\":5}}\n\n",
                    "data: [DONE]\n\n",
                ),
            ),
            (
                Protocol::Anthropic,
                concat!(
                    "data: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":3,\"output_tokens\":0}}}\n\n",
                    "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"he\"}}\n\n",
                    "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"llo\"}}\n\n",
                    "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":2}}\n\n",
                ),
            ),
            (
                Protocol::Responses,
                concat!(
                    "data: {\"type\":\"response.output_text.delta\",\"output_index\":0,\"delta\":\"he\"}\n\n",
                    "data: {\"type\":\"response.output_text.delta\",\"output_index\":0,\"delta\":\"llo\"}\n\n",
                    "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"usage\":{\"input_tokens\":3,\"output_tokens\":2,\"total_tokens\":5}}}\n\n",
                ),
            ),
        ];

        for (protocol, reply) in replies {
            let Streamed { done, text, .. } = ask(protocol, reply).await.unwrap();
            assert_eq!(done.text, "hello", "{protocol:?} lost the reply text");
            assert_eq!(text, ["he", "llo"], "{protocol:?} did not report the deltas");
            let usage = done.usage.unwrap_or_else(|| panic!("{protocol:?} reported no usage"));
            assert_eq!((usage.prompt_tokens, usage.completion_tokens), (3, 2), "{protocol:?} lost usage");
        }
    }

    #[tokio::test]
    async fn every_protocol_streams_reasoning_apart_from_the_reply() {
        let replies = [
            (
                Protocol::OpenAi,
                concat!(
                    "data: {\"choices\":[{\"index\":0,\"delta\":{\"reasoning_content\":\"think\"}}]}\n\n",
                    "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"hi\"}}]}\n\n",
                    "data: [DONE]\n\n",
                ),
            ),
            (
                Protocol::Anthropic,
                concat!(
                    "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"thinking\",\"thinking\":\"\",\"signature\":\"\"}}\n\n",
                    "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"think\"}}\n\n",
                    "data: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"text_delta\",\"text\":\"hi\"}}\n\n",
                ),
            ),
            (
                Protocol::Responses,
                concat!(
                    "data: {\"type\":\"response.reasoning_summary_text.delta\",\"delta\":\"think\"}\n\n",
                    "data: {\"type\":\"response.output_text.delta\",\"output_index\":0,\"delta\":\"hi\"}\n\n",
                ),
            ),
        ];

        for (protocol, reply) in replies {
            let Streamed { done, reasoning, text } = ask(protocol, reply).await.unwrap();
            assert_eq!(reasoning, ["think"], "{protocol:?} lost its reasoning deltas");
            assert_eq!(text, ["hi"], "{protocol:?} mixed reasoning into the reply");
            assert_eq!(done.text, "hi", "{protocol:?} mixed reasoning into the reply text");
        }
    }

    #[tokio::test]
    async fn every_protocol_fails_a_reply_that_was_cut_off() {
        let replies = [
            (
                Protocol::OpenAi,
                "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"length\"}]}\n\ndata: [DONE]\n\n",
                "length",
            ),
            (
                Protocol::Anthropic,
                "data: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"max_tokens\"},\"usage\":{\"output_tokens\":2}}\n\n",
                "max_tokens",
            ),
            (
                Protocol::Responses,
                "data: {\"type\":\"response.incomplete\",\"response\":{\"status\":\"incomplete\",\"incomplete_details\":{\"reason\":\"max_output_tokens\"}}}\n\n",
                "max_output_tokens",
            ),
        ];

        for (protocol, reply, reason) in replies {
            let err = ask(protocol, reply).await.unwrap_err();
            assert!(err.to_string().contains(reason), "{protocol:?} reported {err}");
        }
    }

    #[tokio::test]
    async fn every_protocol_assembles_the_same_tool_call() {
        let replies = [
            (
                Protocol::OpenAi,
                concat!(
                    "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"c1\",\"function\":{\"name\":\"read_file\",\"arguments\":\"{\\\"path\\\":\"}}]}}]}\n\n",
                    "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"\\\"a.txt\\\"}\"}}]}}]}\n\n",
                    "data: [DONE]\n\n",
                ),
            ),
            (
                Protocol::Anthropic,
                concat!(
                    "data: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"c1\",\"name\":\"read_file\",\"input\":{}}}\n\n",
                    "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"path\\\":\"}}\n\n",
                    "data: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"\\\"a.txt\\\"}\"}}\n\n",
                ),
            ),
            (
                Protocol::Responses,
                concat!(
                    "data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"type\":\"function_call\",\"call_id\":\"c1\",\"name\":\"read_file\",\"arguments\":\"\"}}\n\n",
                    "data: {\"type\":\"response.function_call_arguments.delta\",\"output_index\":0,\"delta\":\"{\\\"path\\\":\"}\n\n",
                    "data: {\"type\":\"response.function_call_arguments.delta\",\"output_index\":0,\"delta\":\"\\\"a.txt\\\"}\"}\n\n",
                ),
            ),
        ];

        for (protocol, reply) in replies {
            let Streamed { done, .. } = ask(protocol, reply).await.unwrap();
            assert_eq!(done.tool_calls.len(), 1, "{protocol:?}");
            let call = &done.tool_calls[0];
            assert_eq!((call.id.as_str(), call.function.name.as_str()), ("c1", "read_file"), "{protocol:?}");
            assert_eq!(call.function.arguments, "{\"path\":\"a.txt\"}", "{protocol:?}");
        }
    }
}
