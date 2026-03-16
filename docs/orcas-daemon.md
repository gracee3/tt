# Orcas Daemon

## Role

`orcasd` is the long-lived local Orcas process.

It owns:

- the live Codex WebSocket connection
- local client fanout
- daemon/runtime status
- live thread/session state
- active turn registry and attachment decisions
- snapshot/query responses for frontends
- recent event buffering
- thread metadata persistence hooks

It does not render UI and it does not expose raw Codex WebSocket traffic directly to frontends.

## Lifecycle Separation

There are two distinct lifecycles:

## Upstream Codex lifecycle

- connect to an existing Codex app-server
- or spawn/manage one if configured
- keep reconnect/backoff logic inside the daemon

## Local frontend lifecycle

- supervisor CLI can connect and exit
- TUI can connect and exit
- future browser/backend can connect and exit

Frontend disconnects must not own or tear down the upstream connection.

## Spawn Model

In development, `orcas supervisor daemon start` ensures an `orcasd` binary exists and backgrounds it.

The current process manager:

- resolves `target/debug/orcasd`
- backgrounds it with `setsid`
- writes logs to `~/.local/share/orcas/logs/orcasd.log`
- writes runtime metadata to `${XDG_RUNTIME_DIR:-~/.local/share/orcas/runtime}/orcas/orcasd.json`
- waits for the UDS socket to become reachable

`supervisor daemon restart` is now a first-class workflow. It intentionally replaces the active daemon instead of silently reusing it.
`supervisor daemon stop` now uses a real Orcas IPC shutdown path instead of an external kill.

## Socket Behavior

Default socket location:

- `${XDG_RUNTIME_DIR:-~/.local/share/orcas/runtime}/orcas/orcasd.sock`

Adjacent runtime metadata file:

- `${XDG_RUNTIME_DIR:-~/.local/share/orcas/runtime}/orcas/orcasd.json`

Daemon startup behavior:

- create runtime directories
- remove a stale socket file if no server is actually listening
- remove stale runtime metadata if the recorded PID is gone
- fail if another active daemon already owns the socket

Daemon restart behavior:

- inspect the live socket first
- prefer daemon-reported PID/runtime metadata when available
- fall back to the socket owner PID for older daemons that predate runtime metadata
- stop the old daemon
- clean stale socket/metadata files
- start the new daemon and wait for the socket to become reachable

Daemon stop behavior:

- accept `daemon/stop` over Orcas IPC
- send a successful response
- stop accepting new work
- exit the daemon process
- remove the socket and runtime metadata on shutdown

Current policy is simple and explicit:

- shutdown is immediate
- connected clients observe disconnect
- there is no graceful drain of in-flight client work yet

## Event Fanout

Each local client connection gets:

- request/response handling over the socket
- optional event subscription
- a bounded outgoing queue

If a client is too slow:

- the daemon drops notifications for that client instead of blocking the service

This keeps one bad frontend from stalling the whole local broker.

## Snapshot And Query Layer

`orcasd` now exposes a frontend bootstrap surface:

- `state/get`
- `session/get_active`
- `thread/get`
- `turns/list_active`
- `turn/get`
- `turn/attach`
- `turns/recent`

The intended client flow is:

1. fetch one Orcas-owned snapshot
2. optionally fetch a focused thread view
3. subscribe to live daemon events

This keeps frontend state initialization deterministic and avoids rebuilding UI state from raw upstream events alone.

## Active Turn Model

Active turns are now a first-class Orcas daemon concept rather than just a thread-summary hint.

Each tracked turn carries Orcas-owned state such as:

- `thread_id`
- `turn_id`
- normalized lifecycle state
- `attachable`
- `live_stream`
- recent output snippet when available
- recent event summary when available
- last update timestamp
- terminal/error information when known

Current lifecycle states are:

- `active`
- `completed`
- `failed`
- `interrupted`
- `lost`
- `unknown`

Attachment semantics are intentionally conservative:

- live attachment is daemon-instance scoped
- `turn/attach` succeeds only when the current `orcasd` instance still owns provable live turn state
- after daemon replacement, Orcas may still return terminal or cached turn state, but `attachable` remains false unless the daemon can prove continuity
- `lost` means Orcas previously owned the turn but lost continuity
- `unknown` means Orcas can query or cache the turn, but cannot prove live attachment in the current daemon instance

Current consumer surfaces for this model:

- supervisor turn inspection commands for operator/debug visibility
- supervisor streaming recovery after reconnect
- TUI status/detail projections and in-flight indicators

## Current Status Model

`daemon/status` reports:

- socket path
- runtime metadata path
- configured Codex endpoint
- configured Codex binary path
- upstream connection state
- connected local client count
- known thread count
- daemon PID
- daemon startup time
- daemon version
- daemon build fingerprint
- daemon binary path
- expected local binary fingerprint for comparison

This is enough to answer:

- which daemon is actually running?
- is the active daemon using the current local `target/debug/orcasd`?
- is the socket stale or held by an older daemon instance?

The status view is Orcas-owned and intentionally small.

## Frontend Expectations During Restart

Supervisor commands now split into two behaviors:

- one-shot admin requests reconnect per invocation and keep retry logic short
- streaming prompt/turn flows use a supervisor-side session helper with bounded reconnect

For streaming supervisor commands, the recovery model is:

1. detect daemon disconnect
2. reconnect to the Orcas IPC socket with bounded backoff
3. fetch `state/get`
4. call `turn/attach` for the in-flight turn
5. decide whether the stream can honestly resume, whether only terminal/cached turn state can be recovered, or whether the stream must be reported as interrupted

This is intentionally snapshot-first. Orcas does not claim uninterrupted upstream Codex execution after daemon replacement unless the refreshed Orcas state proves the turn is still live.

The TUI now:

- shows disconnected/reconnecting state when the daemon event stream dies or IPC calls fail
- retries connection with bounded backoff
- reboots from a fresh `state/get` snapshot
- recreates `events/subscribe` after snapshot recovery

Recovery is intentionally snapshot-first. The TUI does not try to infer missed daemon state only from event gaps.
