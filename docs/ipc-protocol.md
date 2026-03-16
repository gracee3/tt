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
- `state/get`
- `session/get_active`
- `models/list`
- `threads/list`
- `threads/list_scoped`
- `thread/start`
- `thread/read`
- `thread/get`
- `thread/resume`
- `turns/recent`
- `turn/start`
- `turn/interrupt`
- `events/subscribe`

## Notifications

Current method:

- `events/notification`

Notification payload:

- one Orcas-owned daemon event envelope

Current event kinds:

- upstream status changed
- session changed
- thread updated
- turn updated
- item updated
- output delta
- warning

## Snapshots

Frontends should now bootstrap with `state/get`, then switch to `events/subscribe` for live updates. `events/subscribe` can still return an initial snapshot when requested.

Current snapshot content:

- daemon status
- active session state
- known thread summaries
- one active/focused thread view when available
- recent event ring buffer

Additional query helpers now include:

- `thread/get`
- `turns/recent`
- `session/get_active`

That gives new clients a deterministic bootstrap path before live notifications begin.

## Design Rules

- do not expose raw Codex wire payloads unless there is a strong reason
- keep method names stable and narrow
- prefer Orcas-owned summaries/views over mirroring the full upstream schema
- let frontends query daemon-owned live state rather than reconstructing everything from stream noise
- add new methods incrementally as Orcas service needs become clear

## Backpressure Behavior

Per-client notification queues are bounded.

Current behavior:

- request/response traffic is prioritized through the active socket
- notification fanout uses bounded queues
- slow clients may miss notifications rather than stall the daemon

This is deliberate for daemon safety in the first pass.
