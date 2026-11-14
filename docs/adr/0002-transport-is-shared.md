# 0002 — Transport is shared; dialects are not

**Status:** Accepted

## Context

thoth speaks three model protocols. Each client was written beside the others and each rebuilt
the same machinery in its own idiom:

- the same six-line stream driver appeared verbatim in two clients and was inlined, differently,
  in the third;
- the same partial-tool-call accumulator was declared three times, with the same conversion to a
  finished call written three times;
- the same "system, then user, then assistant with reasoning before text before tool calls, then
  tool results" lowering was written twice with mirrored matches;
- the same eight request-body fields were declared three times.

The result was that a fix to streaming had to be made in three places, and nothing reported when
one was missed. The Anthropic client in particular grew to 863 lines with a single public
function.

At the same time, the protocols genuinely differ: one sends reasoning as signed blocks, one sends
it as encrypted items, one has a `[DONE]` sentinel, and their tool-call shapes are incompatible.
Sharing everything would mean pretending those differences are not real.

## Decision

Share transport, keep dialects.

- One stream driver owns the sequence: post, iterate events, apply each event to a per-protocol
  state, finish, and check for a cut-off. The terminating condition lives there once.
- Each protocol supplies a stream-state adapter. It sees events and produces deltas; it never owns
  the loop.
- One tool-call buffer accumulates partial calls for every protocol.
- Conversation lowering is a shared skeleton with four per-dialect hooks: system, user, assistant
  ordering, and tool result.
- The request-body fields common to all three are flattened into each dialect's body. The bodies
  themselves are not unified.

## Consequences

A streaming fix lands once. A fourth protocol costs one adapter file rather than a fourth copy of
the loop.

The risk moves rather than disappearing: the driver must not assume anything true of only one
protocol. The known hazards are the terminator, the cut-off check, and whether usage arrives per
event or at the end. Those are the parts to test per adapter, and the reason each adapter keeps
its own conformance test.

Non-streaming response types are deleted. The clients are streaming-only, and the response types
were referenced only by the tests of the file that declared them. If a non-streaming path is ever
needed it will be reintroduced against a real caller.

## What this rules out

A generic "provider client" that takes a dialect at runtime. The differences are structural, not
configuration, and a runtime dispatch would put every protocol's quirks back in one function —
which is the thing this decision is trying to avoid.
