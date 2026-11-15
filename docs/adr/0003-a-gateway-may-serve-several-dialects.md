# 0003 — A gateway may serve several dialects at different paths

**Status:** Accepted

## Context

A provider is one record with one protocol and one base URL (ADR 0001). That fits a service that
speaks one dialect at one address, and it fits Ollama by accident: it serves the OpenAI dialect
under `/v1` and the Anthropic dialect at the root, so `base_url_for` already rewrites one path into
another. GitHub Copilot already rewrites its base URL from its token.

OpenCode Go is the same shape taken further. One subscription serves four dialects:

| Dialect | Base |
| --- | --- |
| chat completions | `https://opencode.ai/inference/go/openai/v1` |
| responses | `https://opencode.ai/inference/go/openai/v1` |
| messages | `https://opencode.ai/inference/go/anthropic` |
| generate content | `https://opencode.ai/inference/go/google` |

and its model list says which dialect each model speaks: one `api` key per model
(`chat-completions`, `responses`, `messages`, `generate-content`). So the provider's default
protocol is a fallback, not a fact. The same list at `GET /v1/models` also carries the model's
display name and context limit, which the plain OpenAI `/models` shape does not.

Meanwhile `Protocol::Google` had a provider, a credential header, and a model fetch, but no client:
`Client::for_model` refused it. Go's only Gemini model could not work until that client existed.

## Decision

A provider is still one record. Two of its fields may vary along the protocol axis:

- **The base URL may be rewritten per protocol.** `Provider::base_url_for(protocol)` is the one
  place that knows where each dialect lives on a provider. A gateway that serves several dialects
  stores its root URL; a provider that serves one stores the address it uses.
- **A model may name its own protocol.** `ModelSpec::protocol()` prefers the model's protocol and
  falls back to its provider's. The model fetch fills it in when the provider's list says which
  dialect a model speaks.

When a provider's model list is not the OpenAI `{"data":[{"id":…}]}` shape, `ModelFetcher` gets one
branch keyed by provider id, beside the existing Copilot branch. The branch maps the provider's
dialect keys onto `Protocol`, keeps the context limit it reports, and drops models it cannot serve.
A second gateway with a rich list promotes that branch to data; one is a branch.

The OpenCode Go record is therefore:

- id `opencode-go`, display name OpenCode Go, API key from `OPENCODE_API_KEY`;
- base URL `https://opencode.ai/inference/go`, with `base_url_for` deriving `/openai/v1` for chat
  completions and responses, `/anthropic` for messages, and `/google` for generate content;
- models from `GET /v1/models`, each carrying the protocol of its `api` key, its name, and its
  context limit.

A provider may also name the header its requests carry a session's id in (ADR 0004), which is how
`x-opencode-session` reaches the gateway without a provider id appearing in the auth layer.

## Consequences

Go's GPT, Claude, Qwen and Gemini models are reachable in their native dialect instead of being
translated into one lowest-common-denominator shape, so a model's reasoning, tool calls and usage
arrive in the form its own client already tests.

The path layout of a gateway now lives in `base_url_for`, not in `models-store.json`. A user can
still point the record at a proxy by editing the root, but a gateway that changes its path layout
needs a code change. That is the price of one record per provider rather than one per dialect.

The fetch branch is provider knowledge in code, not data. It is accepted on the same terms as the
Copilot branch: one is a branch, two become a field, and the fixture test pins the body shape.

## What this rules out

One provider record per dialect for the same gateway. It would duplicate the credential, make a
model spec's provider id depend on the dialect the model happens to speak, and make switching
between two models of the same subscription look like switching subscriptions.

A translation layer that sends every model of a gateway through the chat-completions dialect. It
works until the model's reasoning or tool-call shape is the thing being tested.
