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
- `daemon/stop`
- `state/get`
- `session/get_active`
- `models/list`
- `threads/list`
- `threads/list_scoped`
- `thread/start`
- `thread/read`
- `thread/get`
- `thread/resume`
- `turns/list_active`
- `turns/recent`
- `turn/get`
- `turn/attach`
- `turn/start`
- `turn/interrupt`
- `workstream/create`
- `workstream/list`
- `workstream/get`
- `workunit/create`
- `workunit/list`
- `workunit/get`
- `assignment/start`
- `assignment/get`
- `report/get`
- `report/list_for_workunit`
- `decision/apply`
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

Reconnect uses the same ordering:

1. reconnect IPC transport
2. fetch `state/get`
3. recreate `events/subscribe`

This is the canonical recovery path.

Current snapshot content:

- daemon status
- active session state
- known scoped thread summaries
- one active/focused thread view when available
- recent event ring buffer

Additional query helpers now include:

- `thread/get`
- `turns/list_active`
- `turn/get`
- `turn/attach`
- `turns/recent`
- `session/get_active`
- `workstream/get`
- `workunit/get`
- `assignment/get`
- `report/get`

Current collaboration helpers now include:

- `workstream/create`, `workstream/list`, `workstream/get`
- `workunit/create`, `workunit/list`, `workunit/get`
- `assignment/start`, `assignment/get`
- `report/get`, `report/list_for_workunit`
- `decision/apply`

That gives new clients a deterministic bootstrap path before live notifications begin.

Current daemon status payload also includes:

- runtime metadata path
- daemon PID
- daemon startup timestamp
- daemon version
- daemon build fingerprint
- daemon binary path

Current thread summaries also include Orcas-owned frontend fields:

- `scope`
- `recent_output`
- `recent_event`
- `turn_in_flight`

Current turn query payloads include Orcas-owned turn lifecycle and attachment fields:

- normalized lifecycle state
- `attachable`
- `live_stream`
- recent output snippet
- recent event summary
- last update timestamp

Important rule:

- live `turn/attach` is daemon-instance scoped
- after daemon replacement, `turn/get` or `turn/attach` may still return terminal or cached turn state, but `attached` is only true when the current daemon instance can still prove live continuity

Current local consumers of this turn contract:

- supervisor `turns list-active`
- supervisor `turns get --thread ... --turn ...`
- TUI turn-aware status/detail projections

Current collaboration loop semantics:

- `assignment/start` binds one work unit to one worker and one worker session
- worker execution is Codex-backed, but the report is stored as an Orcas-owned object
- `decision/apply` records the supervisor decision explicitly
- `continue` and `redirect` create a new pending assignment for the same work unit rather than mutating one assignment forever

## Design Rules

- do not expose raw Codex wire payloads unless there is a strong reason
- keep method names stable and narrow
- prefer Orcas-owned summaries/views over mirroring the full upstream schema
- let frontends query daemon-owned live state rather than reconstructing everything from stream noise
- treat subscriptions as invalid after disconnect and recreate them after snapshot recovery
- add new methods incrementally as Orcas service needs become clear

## Backpressure Behavior

Per-client notification queues are bounded.

Current behavior:

- request/response traffic is prioritized through the active socket
- notification fanout uses bounded queues
- slow clients may miss notifications rather than stall the daemon

This is deliberate for daemon safety in the first pass.
