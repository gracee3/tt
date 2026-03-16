# Orcas Daemon

## Role

`orcasd` is the long-lived local Orcas process.

It owns:

- the live Codex WebSocket connection
- local client fanout
- daemon/runtime status
- live thread/session state
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
- waits for the UDS socket to become reachable

## Socket Behavior

Default socket location:

- `${XDG_RUNTIME_DIR:-~/.local/share/orcas/runtime}/orcas/orcasd.sock`

Daemon startup behavior:

- create runtime directories
- remove a stale socket file if no server is actually listening
- fail if another active daemon already owns the socket

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
- `turns/recent`

The intended client flow is:

1. fetch one Orcas-owned snapshot
2. optionally fetch a focused thread view
3. subscribe to live daemon events

This keeps frontend state initialization deterministic and avoids rebuilding UI state from raw upstream events alone.

## Current Status Model

`daemon/status` reports:

- socket path
- configured Codex endpoint
- configured Codex binary path
- upstream connection state
- connected local client count
- known thread count

The status view is Orcas-owned and intentionally small.
