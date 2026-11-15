# thoth

Thoth is a terminal AI agent: a person talks to a model that can call tools in a workspace.
This glossary fixes the words the code and the specs use, so that a term means one thing in a
commit message, an issue, and a test name.

## The agent

**Agent**:
A persona given a conversation, a model, and a set of tools. It runs one turn at a time until the
user stops talking to it.
_Avoid_: assistant, bot, worker

**Persona**:
A named configuration an Agent is built from: a key, a system prompt, an optional model and
thinking level, and which tools and skills it may use. Personas live on disk.
_Avoid_: profile, character, role

**Sub-agent**:
An agent started by another agent through the delegate tool. It has its own conversation, inherits
the delegating agent's settings, and cannot delegate further.
_Avoid_: child agent, worker, thread

**Turn**:
One user prompt and everything the agent does for it. A turn that fails without having changed
anything outside the conversation is discarded, so the conversation reads as if it never started;
one that edited files is kept, so the next turn knows about the edits.
_Avoid_: round, cycle, request

**Step**:
One model call inside a turn. A turn is bounded by a maximum number of steps, and a step that asks
for tools is what makes the turn continue.
_Avoid_: iteration, loop

**Budget**:
How much context a conversation may use: when to summarise old turns, and when to drop old tool
output. It is a property of the conversation, not an allowance for one turn.
_Avoid_: limit, quota

**Compaction**:
Replacing older turns with a short summary once the conversation passes the compaction threshold,
which is the lower of the budget and the model's share of its context window.
_Avoid_: summarisation, truncation

**Elision**:
Replacing old tool output with a short note, keeping the newest bulky output in full.
_Avoid_: truncation, pruning

**Skill**:
A written instruction file an agent can load on demand through the invoke tool, instead of carrying
it in every prompt.
_Avoid_: plugin, extension, prompt

**Agent context**:
What an agent knows before the first prompt: the system prompt, the instruction files that apply
to the workspace, the working directory, and the budget.
_Avoid_: context, configuration

**Context window**:
How many tokens a model accepts in one request. Not the agent context, and not a tool's context.
_Avoid_: context, window

## Models and providers

**Provider**:
A service that offers models, built in or configured by the user. It owns an id, a login method,
and a base URL that may differ per protocol.
_Avoid_: backend, vendor, endpoint

**Model**:
One model offered by a provider, with the context limit and metadata that provider reports.
_Avoid_: LLM, engine

**Model spec**:
A provider and a model id together, which is what identifies a model to talk to. The same model
name at two providers is two different models.
_Avoid_: model reference, model key, model id

**Protocol**:
The wire dialect a provider speaks. A model may name its own; otherwise it inherits its provider's.
_Avoid_: API, format

**Login method**:
How a provider is authenticated: not at all, an API key, or a device flow.
_Avoid_: auth type, login

**Credential**:
The stored secret for one provider — an API key or an OAuth token. A login produces one; the
credential store keeps it.
_Avoid_: token, secret

## Tools

**Tool**:
A function the model may call, with a name, a description, and an argument schema. Tools are the
only way an agent acts on the world.
_Avoid_: command, action, function

**Tool traits**:
What the agent loop may assume about a tool beyond its schema — today only whether it is read-only.
The default is the cautious one.
_Avoid_: metadata, flags, capabilities

**Tool result**:
What a tool call returns: content for the model, whether it failed, and any tokens a sub-agent
spent.
_Avoid_: output, response, outcome

**Read-only**:
A tool property meaning its calls change nothing, so calls like it may run at the same time.
_Avoid_: safe, pure, idempotent

**Tool context**:
What a running tool call is given: its call id, the turn's cancellation, the agent's settings, the
skills, and a way to report progress.
_Avoid_: context, env, runtime

**Workspace**:
The working directory tools may touch.
_Avoid_: sandbox, root, cwd

## The conversation

**Transcript**:
The ordered record of what the chat shows: messages and tool cards, each identified by an id that
edits refer to.
_Avoid_: history, log

**Transcript change**:
One edit to the transcript: an item added, an item appended to, an item replaced, or the whole
transcript cleared.
_Avoid_: event, update, delta

**Speaker**:
Who a transcript message is from — the user, the agent, its thinking, a tool, a notice, or one of
the app's own notices.
_Avoid_: author, role

**Notice**:
A message the app says to the user, rather than the model or the agent.
_Avoid_: alert, warning

**Tool card**:
The transcript entry that shows one tool call: its summary, its status, and its output.
_Avoid_: block, widget

**View**:
What a client is shown of one session: whether the agent is busy, its settings, its token
statistics, and its transcript.
_Avoid_: state, snapshot

## What the layers report

Each layer reports its own events, and they are never the same thing. Say which: *agent event*,
*session event*, *harness event*.

**Agent event**:
What a running turn reports as it goes: streamed text, reasoning, a step starting, a tool starting,
progressing or finishing, and notices.
_Avoid_: event, update

**Session event**:
What one session reports to its client: busy or idle, settings changed, statistics changed, the
transcript changed.
_Avoid_: event, notification

**Harness event**:
What the shared runtime reports: a login changing, a notice, an event from one session, or a
session closing.
_Avoid_: event, message

**Agent change**:
An instruction applied to an agent: clear the conversation, interrupt the running turn, set the
model, set the thinking level.
_Avoid_: command, action, mutation

## The terminal

**Session**:
One conversation and its view: the agent behind it, what the client may show, and the statistics
for it. It is identified by a session id.
_Avoid_: chat, tab, window

**Session id**:
The identity of a session: a ULID minted when it starts, independent of its model and of the client
showing it. It survives clearing the conversation and switching models, is sent to a gateway that
asks for one, and is the key a session will be stored under.
_Avoid_: conversation id, thread id, cache key

**Harness**:
The shared runtime a client talks to: every open session, plus the builder, credentials, models,
personas, and login state those sessions share.
_Avoid_: server, manager, runtime, test rig

**Command**:
Something the user types after a slash. Commands are user-facing and unrelated to tools.
_Avoid_: action, verb

**Choice**:
One option a command offers, such as one of the models after `/model`.
_Avoid_: item, entry, option

**Picker**:
The popup list that filters as the user types and completes or runs what they choose.
_Avoid_: menu, dropdown, autocomplete

**Completion**:
The text a picker inserts when the user accepts a row.
_Avoid_: suggestion, result

**Scope**:
Whether accepting a completion replaces the whole prompt or only the mention being typed.
_Avoid_: mode, target

**Mention**:
A file reference the user starts with the trigger character.
_Avoid_: reference, link

**Selection**:
Which tools or skills a persona may use: all of them, or a named few. Unrelated to selecting text
with the mouse.
_Avoid_: filter, subset
