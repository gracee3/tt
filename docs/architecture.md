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
- recent-event snapshotting
- daemon status reporting
- shared IPC client/process manager used by frontends

`orcasd` is now the single owner of live upstream Codex state.

## `orcas-supervisor`

Thin operational CLI client:

- starts or reuses `orcasd`
- sends narrow IPC requests
- streams turn output from Orcas IPC events
- no longer owns a direct Codex client

## `orcas-tui`

Minimal proof-of-boundary frontend:

- attaches to `orcasd`
- shows daemon connection status
- shows thread summaries
- shows event log

It is intentionally shallow in this pass.

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

## Event Model

Codex notifications are translated into Orcas-owned `EventEnvelope` values.

Current emitted events include:

- connection state changes
- thread started/status changed
- turn started/completed
- item started/completed
- agent message deltas
- warning events

`orcasd` keeps:

- a broadcast bus for live subscribers
- a small in-memory recent-event ring buffer for snapshot-on-subscribe

This gives new clients a small initial picture without replaying the full world.

## Persistence Model

Persistence is intentionally lightweight:

- config: TOML
- state: JSON
- logs: plain text

Stored now:

- configured Codex endpoint/binary via config
- known Orcas thread metadata in `state.json`
- runtime socket and daemon logs under XDG paths

This stays behind `OrcasSessionStore` so a future SQLite backend can replace it.

## Current Rough Edges

- `threads/list` still mirrors the broad upstream thread set
- no richer Orcas-side query model yet
- no dedicated approval UX
- no auth or multi-user model
- no browser bridge yet
- daemon process management currently assumes a Unix-like environment with `setsid`

## Expansion Path

The intended next layer is not more raw protocol.

The intended next layer is richer Orcas-owned service state inside `orcasd`, for example:

- thread registry views scoped to Orcas
- recent turn history
- resumable active-session state
- approval workflow surfaces
- browser/backend attachment using the same IPC contract shape
