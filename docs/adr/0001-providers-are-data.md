# 0001 — Providers are data, not an enum

**Status:** Accepted

## Context

Providers began as a five-variant enum: Ollama, OpenAI, Anthropic, GitHub Copilot, Google. Each
variant carried its own base URL, protocol, and login method in a match statement.

Two things broke that shape. Users wanted to point thoth at gateways we do not ship — Ollama
behind a proxy, an OpenAI-compatible endpoint, a local model server. And the enum made every
provider-shaped question a match: the model store, the auth layer, the picker ordering, and the
request builder each answered "which provider is this" by re-matching the same five variants.

The migration to a data record was started and left half-finished, which is why the workspace
did not compile and why three representations of a provider coexisted: a string id, a struct, and
an enum variant.

## Decision

A provider is a record, identified by a string `id`.

- `id` is the identity. `Eq`, `Hash`, and the key in `models-store.json` all use it. Two providers
  equal on `id` are the same provider, whatever their cached model lists say.
- Everything else — display name, login method, env var, protocol, base URL, headers, checked-at
  timestamp, cached models — is data that the user may override on disk.
- The built-in providers are defaults, not special cases. They are ordinary records created by
  `Provider::builtin(id)`, seeded into the store when it is empty.
- Ordering is data too: the two id arrays that decide menu order live next to the providers, not in
  the layer that draws the menu.

## Consequences

Adding a provider is an id array entry plus a record — one file, no new match arm anywhere.

The model store interface follows: fetch one by id, list all, save one. There is no second
serialisation shape, because the record *is* the stored shape. A legacy `models-store.json` with
the old envelope does not parse and falls into the existing "could not read, run /refresh-models"
path rather than being migrated — one fewer schema to carry forever.

Auth resolution has to happen at one point per request, because a provider's effective base URL
is not always its stored one: GitHub Copilot rewrites it from its token. `Auth::for_model` takes a
`ModelSpec` and resolves protocol, base URL, and header together, which is the only place that
knows all three.

The cost is that a provider is cloned by value where an enum was `Copy`, and that nothing stops
a caller from constructing one with inconsistent fields. Both are acceptable: the record is built
by `builtin`, `custom`, or the deserialiser, and those are the only constructors.

## What this rules out

Reintroducing an enum variant for a provider we happen to ship, or for one that needs behaviour
the others do not. If Copilot needs something special, it is a field or a function on the record,
not a variant that forces every other provider's match to grow an arm.
