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
- writes runtime metadata to `${XDG_RUNTIME_DIR:-~/.local/share/orcas/runtime}/orcas/orcasd.json`
- waits for the UDS socket to become reachable

`supervisor daemon restart` is now a first-class workflow. It intentionally replaces the active daemon instead of silently reusing it.

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

Supervisor commands reconnect per invocation, so they tolerate daemon restart naturally.

The TUI now:

- shows a disconnected/error banner when the daemon event stream dies
- marks upstream state as disconnected on backend failure
- can recover by refreshing, which fetches a fresh snapshot and resubscribes to events

This is intentionally explicit. Full background reconnect is deferred.
