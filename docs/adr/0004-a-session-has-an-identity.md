# 0004 — A session has an identity independent of its model and its client

**Status:** Accepted

## Context

A session had no identity of its own. Three weaker things stood in for one:

- the harness keyed sessions by a `u64` that started at 1 and was reused once a session closed, so
  the number named a slot, not a conversation;
- each client minted a random `prompt_cache_key` when it was built, and `Agent::set_model` builds a
  new client on every model switch, so the key changed mid-conversation;
- a provider that wanted a per-conversation identifier had nothing to send.

OpenCode Go asks its clients for a stable session id in `x-opencode-session`, for routing and
prompt caching. Session storage will need the same identifier later. Neither can be built on a
reused slot number or on a per-client random key.

## Decision

A session is identified by a `SessionId`: a newtype around a ULID, minted when the session starts
by a generator the harness owns.

- The id is the session's key in the harness, the id in session events, and what the terminal
  addresses a session by. No numeric session id remains.
- The harness draws ids from one `ulid::Generator`, so ids sort in creation order within the
  process, which is the order the terminal's session switching expects.
- The id is opaque and displayed as its bare 26-character ULID; it is usable as a filename and as a
  header value. It is not derived from the model, the contents, or the title.
- The id survives clearing the conversation and switching models: it identifies the session, not
  its contents or its model.
- The agent carries the id and hands it to each client it builds, which passes it to `Auth`. `Auth`
  sends it as the prompt-cache key where a protocol has one, and as a header when the provider names
  that header (`sessionHeader`, today only `x-opencode-session` on `opencode-go`).
- Each sub-agent mints its own id. A sub-agent is a conversation of its own, even though it runs
  inside the same session.

Persistence is not part of this decision. The id is minted in memory and nothing reads it back yet;
when sessions are stored, this is the key they are stored under. A `parent_id` linking a sub-agent
to the session that started it is likewise deferred until tracing exists, and will be added next to
`SessionId` rather than in place of it.

## Consequences

The terminal compares opaque ids instead of literals, and tests that asserted "session 2" assert
the id they were handed.

The prompt-cache key stops being a client concern: a client no longer generates one, and a model
switch no longer resets it. `new_cache_key` and the two client fields that held it are deleted.

A provider that needs a different header gets a field, not an `if` on its id, which keeps ADR 0001's
promise that adding a provider is data.

Two sessions created in the same millisecond still differ, because the ULID's random tail is drawn
from a generator that also keeps them ordered.

## What this rules out

A per-client or per-request identity. Every request of a session carries the same id, and every
sub-agent's requests carry its own.

Minting a new id on `/clear`. Clearing changes what the session contains, not which session it is.

Storing the id before there is a store. A field that nothing reads is a field that drifts.
