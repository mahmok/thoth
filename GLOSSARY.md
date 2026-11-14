# thoth

Thoth is a terminal AI agent: a person talks to a model that can call tools in a workspace.
This glossary fixes the words the code and the specs use, so that a term means one thing in a
commit message, an issue, and a test name.

## The agent

**Agent**:
A persona given a conversation, a model, and a set of tools, which the user talks to.
_Avoid_: assistant, bot, worker

**Persona**:
A named configuration — system prompt, model, tools, skills — that an agent is built from.
_Avoid_: profile, character, role

**Sub-agent**:
An agent started by another agent through the delegate tool. It has its own conversation and
cannot delegate further.
_Avoid_: child agent, worker, thread

**Turn**:
One exchange: a prompt, the model's streamed reply, the tool calls it asks for, and the reply that
follows them. A turn ends when the agent answers without calling a tool, or runs out of steps.
_Avoid_: round, cycle, request

**Step**:
One model call inside a turn. A turn is bounded by a maximum number of steps.
_Avoid_: iteration, loop

**Budget**:
The token allowance one turn may spend before older conversation is compacted or elided.
_Avoid_: limit, quota

**Compaction**:
Replacing older turns with a short summary so the conversation fits the context window.
_Avoid_: summarisation, truncation

**Elision**:
Dropping the output of old tool calls the model no longer needs.
_Avoid_: truncation, pruning

## Models and providers

**Provider**:
A service that offers models — a built-in one, or a gateway the user configured. It owns a base
URL, a protocol, and a way to log in.
_Avoid_: backend, vendor, endpoint

**Model**:
One model offered by a provider, with the context limit and metadata that provider reports.
_Avoid_: LLM, engine

**Model spec**:
A provider and a model id together. The same model name at two providers is two different models.
_Avoid_: model reference, model key, model id

**Protocol**:
The wire dialect a provider speaks. A provider chooses one; its models inherit it.
_Avoid_: API, format

**Context window**:
How many tokens a model accepts in one request. Not the agent's context and not a tool's context.
_Avoid_: context, window

**Login method**:
How a provider is authenticated: nothing, an API key, or a device flow.
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
What the agent loop may assume about a tool beyond its schema — today only whether it is
read-only. The default is the cautious one.
_Avoid_: metadata, flags, capabilities

**Tool result**:
What a tool call returns: content for the model, whether it failed, and any tokens a sub-agent
spent.
_Avoid_: output, response, outcome

**Read-only**:
A tool property meaning its calls change nothing, so calls like it may run at the same time.
_Avoid_: safe, pure, idempotent

**Workspace**:
The directory a tool may act in, and the rules for reading and writing inside it.
_Avoid_: sandbox, root, cwd

## The conversation

**Transcript**:
The ordered record of what the chat shows: messages, tool cards, and the changes made to them.
_Avoid_: history, log

**Speaker**:
Who a transcript message is from — the user, the agent, its thinking, a tool, a notice, or one of
the app's own notices.
_Avoid_: author, role

**Notice**:
A message the app says to the user, rather than the model or the agent.
_Avoid_: alert, warning

**Tool card**:
The chat entry that shows one tool call: its summary, its status, and its output.
_Avoid_: block, widget

**Change**:
An instruction applied to an agent — set the model, set the thinking level, clear the conversation,
interrupt.
_Avoid_: event, action, mutation

## The terminal

**Session**:
One conversation with an agent as the terminal sees it: what can be shown, cancelled, compacted,
or closed.
_Avoid_: chat, tab, window

**Harness**:
The terminal's facade over every session: it opens, closes, polls, and routes what the user types.
_Avoid_: server, manager, runtime

**Command**:
Something the user types after a slash. Commands are user-facing and unrelated to tools.
_Avoid_: action, verb

**Choice**:
One option a command offers, such as one of the models after `/model`.
_Avoid_: item, entry, option

**Picker**:
The popup list that filters as the user types and completes or runs what they choose.
_Avoid_: menu, dropdown, autocomplete

**Scope**:
Whether picking completes the whole prompt or just the mention being typed.
_Avoid_: mode, target

**Mention**:
A file reference the user starts with the trigger character.
_Avoid_: reference, link
