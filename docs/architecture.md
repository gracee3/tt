# Orcas Architecture

## Goal

Orcas is structured so Codex protocol churn is isolated behind Orcas-owned boundaries.

The key split is:

- upstream lifecycle: Codex app-server over WebSocket
- local lifecycle: `orcasd` plus one or more Orcas clients over Unix domain socket IPC

Frontends do not own the upstream connection anymore.

## Crate Boundaries

## `orcas-core`

Stable Orcas concepts:

- app config
- runtime paths
- error/result types
- shared event envelope types
- session/thread metadata
- lightweight JSON session store
- Orcas IPC request/response/event types
- Orcas JSON-RPC helpers

`orcas-core` should stay free of Codex transport specifics.

## `orcas-codex`

Unstable integration edge with Codex:

- `CodexTransport`
- WebSocket transport
- request ID management
- reconnect/backoff
- app-server JSON-RPC types
- narrow typed method wrappers
- Codex app-server spawn/status management
- approval routing hook

This is the only place that should absorb most Codex protocol churn.

## `orcas-daemon`

Shared local orchestration service:

- `orcasd` binary
- one persistent upstream Codex connection
- one local IPC server over Unix domain socket
- local client connection handling
- event fanout
- live Orcas-owned state/query snapshots
- recent-event snapshotting
- daemon runtime metadata and build fingerprint reporting
- graceful daemon shutdown over IPC
- daemon status reporting
- shared IPC client/process manager used by frontends

`orcasd` is now the single owner of live upstream Codex state.

## `orcas-supervisor`

Thin operational CLI client:

- starts or reuses `orcasd`
- sends narrow IPC requests
- keeps one-shot admin commands simple and short-lived
- uses a small session-aware streaming helper for prompt/turn flows
- exposes a small turn inspection surface for operator/debug visibility
- streams turn output from Orcas IPC events with bounded reconnect
- no longer owns a direct Codex client

## `orcas-tui`

Testable frontend split:

- backend trait with real daemon IPC adapter and fake test adapter
- reducer-owned `AppState`
- explicit `Action` and `Effect` loop
- `UiEvent` translation layer between daemon events and reducer updates
- selectors/view-models for render and tests
- ratatui render layer as a thin projection over state
- turn lifecycle/attachability projections sourced from Orcas turn state where available

The goal is not UI breadth yet. The goal is one canonical state machine that runs both interactively and headlessly.

## Runtime Topology

```text
supervisor CLI ----\
                    \
TUI -----------------+--> orcasd --(WebSocket JSON-RPC)--> codex app-server
                    /
future browser ----/
```

Important lifecycle rule:

- frontend disconnects must not tear down the upstream Codex connection

## Local IPC Model

Transport:

- Unix domain socket
- newline-delimited JSON messages
- JSON-RPC 2.0 style request/response/notification framing

Design choices:

- local-only by default
- no raw Codex wire exposure as the public Orcas contract
- bounded per-client outgoing queues
- slow client notifications are dropped instead of blocking the daemon

The current Orcas IPC surface is intentionally narrow. See `docs/ipc-protocol.md`.

## Upstream Connection Model

Inside `orcasd`:

- one `CodexClient`
- one reconnecting WebSocket transport
- one initialize handshake path
- one thread cache owned by the daemon

The upstream connection can be backed by:

- an already-running Codex app-server
- a Codex app-server spawned by Orcas if configured

## Event And Query Model

Codex notifications are still received and normalized at the daemon edge, but frontends now consume Orcas-owned state/query and event types instead of raw upstream envelopes.

`orcasd` now keeps:

- a live in-memory thread/session model
- a daemon-owned active-turn registry
- a broadcast bus for live daemon events
- a recent-event ring buffer
- recent per-thread output/event snippets
- a snapshot/query surface for frontend bootstrap

The current frontend bootstrap path is:

1. `state/get`
2. optional follow-up `thread/get`
3. `events/subscribe`

This avoids forcing every client to reconstruct frontend state from raw streaming events and broad upstream thread lists.

The reconnect path now follows the same rule:

1. reconnect transport
2. `state/get`
3. recreate `events/subscribe`

Missed state is recovered from the snapshot, not inferred from event gaps alone.

Turn recovery now has a narrower Orcas-owned contract too:

- `turns/list_active` exposes active attachable turns known to the daemon
- `turn/get` returns Orcas turn lifecycle state and cached output/status when available
- `turn/attach` answers whether the current daemon instance can still prove live attachment for that turn

This is intentionally conservative. Live attachment is daemon-instance scoped, not a claim that upstream Codex execution survives daemon replacement automatically.

That same contract is now consumed in the main local surfaces:

- supervisor can inspect active turns and specific turn state directly
- TUI status/detail projections prefer Orcas turn lifecycle state over thread-level `turn_in_flight` hints when explicit turn data exists

Supervisor streaming commands follow the same snapshot-first rule, but with stricter user-facing semantics:

- if the daemon disappears mid-stream, the supervisor reconnects with bounded backoff
- once reconnected, it re-anchors on `state/get` plus `turn/attach`
- if the target turn is still attachable, the supervisor re-subscribes
- if the target turn is only recoverable as terminal or cached turn state, the supervisor reports that explicitly and prints the recovered suffix when available
- if continuity cannot be proven, the command exits as interrupted instead of implying uninterrupted upstream execution

Current daemon event types include:

- upstream status changes
- session/active turn changes
- thread summary updates
- turn updates
- item updates
- streamed output deltas
- warnings

## Persistence Model

Persistence is intentionally lightweight:

- config: TOML
- state: JSON
- logs: plain text

Stored now:

- configured Codex endpoint/binary via config
- known Orcas thread metadata in `state.json`
- runtime daemon metadata next to the UDS socket
- runtime socket and daemon logs under XDG paths

This stays behind `OrcasSessionStore` so a future SQLite backend can replace it.

## TUI Testing Model

The TUI now has one canonical app core:

- real daemon-backed runtime for interactive use
- fake backend-backed runtime for tests
- no duplicate test-only state model
- no primary reliance on screen scraping

Most TUI tests assert on state and view-model projections. Render validation is intentionally secondary.

## Current Rough Edges

- `threads/list` is still broader than the scoped frontend snapshot
- one-shot supervisor retry logic is intentionally shallow and command-scoped
- supervisor streaming recovery is turn-aware at the Orcas layer, but not upstream-turn durable across daemon replacement
- no dedicated approval UX
- no auth or multi-user model
- no browser bridge yet
- daemon process management currently assumes a Unix-like environment with `setsid`
- end-to-end turn completion still depends on upstream Codex availability

## Expansion Path

The intended next layer is to deepen the Orcas-owned session model rather than widen raw protocol mirroring, for example:

- tighter Orcas-scoped thread views
- richer active session and resume flows
- more automatic frontend reconnect/resubscribe behavior
- approval workflow surfaces
- browser/backend attachment using the same IPC contract shape
