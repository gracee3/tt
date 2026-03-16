# Orcas IPC Protocol

Current local IPC transport:

- Unix domain socket
- newline-delimited JSON messages
- JSON-RPC 2.0 style framing

This contract is Orcas-owned. It is intentionally narrower than the upstream Codex app-server surface.

## Requests

Current methods:

- `daemon/status`
- `daemon/connect`
- `models/list`
- `threads/list`
- `thread/start`
- `thread/read`
- `thread/resume`
- `turn/start`
- `turn/interrupt`
- `events/subscribe`

## Notifications

Current method:

- `events/notification`

Notification payload:

- one Orcas `EventEnvelope`

## Snapshots

`events/subscribe` can request an initial snapshot.

Current snapshot content:

- daemon status
- known thread summaries
- recent event ring buffer

That gives new clients a small initial picture before live notifications begin.

## Design Rules

- do not expose raw Codex wire payloads unless there is a strong reason
- keep method names stable and narrow
- prefer Orcas-owned summaries/views over mirroring the full upstream schema
- add new methods incrementally as Orcas service needs become clear

## Backpressure Behavior

Per-client notification queues are bounded.

Current behavior:

- request/response traffic is prioritized through the active socket
- notification fanout uses bounded queues
- slow clients may miss notifications rather than stall the daemon

This is deliberate for daemon safety in the first pass.
