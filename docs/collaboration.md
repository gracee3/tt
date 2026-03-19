# Orcas Collaboration

## Overview

Orcas keeps collaboration state local and authoritative. The daemon owns the workflow records that matter for supervision, the local IPC contract that frontends use, and the live bridge to the upstream Codex app-server. The CLI and TUI are clients of that daemon rather than direct Codex clients.

This document describes the current collaboration model and the concrete IPC surface that supports it. The architecture overview explains the broader runtime split; this page focuses on the workflow objects, the request/response surface, and the event flow that frontends consume.

The planned local-backend evolution for workstream, work unit, and tracked-thread CRUD is documented in [Local-Authority MVP Backend Design](design/local-authority-mvp-backend.md). The MVP thread semantics decision is captured in [ADR 0001](adr/0001-tracked-thread-is-a-local-binding-record.md).

## Collaboration Model

Orcas models work as a small set of explicit records.

- Workstreams group related work under a shared objective and priority.
- Work units represent concrete tasks inside a workstream.
- Assignments bind a worker session to a work unit and carry the execution instructions and status.
- Reports record the outcome of execution back into Orcas-owned state.
- Decisions record the supervisor outcome for a work unit.
- Proposals are review artifacts that precede a human-approved decision path.
- Threads and turns represent the Codex-side execution view that Orcas supervises.

The important rule is separation. Codex execution history stays in the worker substrate, while Orcas keeps the supervision model, review state, and operator-facing summaries in its own state store.

## IPC Contract

Orcas IPC uses a local Unix domain socket and JSON-RPC 2.0 style messages. Messages are newline-delimited JSON records. Clients issue requests for commands and queries, receive responses for results, and subscribe to notifications for incremental updates.

The daemon exposes a snapshot-first interaction pattern. Clients typically request current state first, then subscribe to live events. That keeps reconnect behavior deterministic and avoids rebuilding UI state from raw event gaps.

Current request families include:

- daemon lifecycle and status:
  - `daemon/status`
  - `daemon/connect`
  - `daemon/stop`
  - `daemon/disconnect`
- snapshot and session state:
  - `state/get`
  - `session/get_active`
- models and thread views:
  - `models/list`
  - `threads/list`
  - `threads/list_scoped`
  - `threads/list_loaded`
  - `thread/start`
  - `thread/read`
  - `thread/read_history`
  - `thread/get`
  - `thread/attach`
  - `thread/detach`
  - `thread/resume`
- turn views and turn control:
  - `turns/list_active`
  - `turns/recent`
  - `turn/get`
  - `turn/attach`
  - `turn/start`
  - `turn/steer`
  - `turn/interrupt`
- collaboration state:
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
- event subscription:
  - `events/subscribe`

Notifications are delivered on `events/notification` with Orcas-owned event envelopes. The daemon keeps a recent event buffer and bounded per-client queues so one slow frontend cannot stall the broker.

## Snapshot And Event Flow

The current client flow is:

1. Connect to the daemon socket.
2. Request `state/get` for a full snapshot.
3. Optionally fetch a focused thread or turn view.
4. Subscribe to `events/subscribe` for live updates.
5. Rebuild the snapshot first after reconnect, then resubscribe.

This flow is used by both the CLI and the TUI. It is intentionally conservative: if the daemon restarts, clients do not assume continuity from missing events alone.

## Upstream Codex Integration

`orcasd` owns the upstream Codex app-server connection. Clients never talk to Codex directly.

The daemon connects to a configured WebSocket endpoint, with a localhost endpoint used by default in the current configuration. The upstream transport details remain an internal implementation concern. Orcas surfaces the resulting thread, turn, and collaboration state through its own IPC contract instead of mirroring the upstream wire format wholesale.

## Operational Consequences

The daemon is the authority for what is active, what is terminal, and what should be shown to operators. That means:

- restarting the CLI or TUI does not affect the upstream worker connection
- restarting the daemon does
- clients should treat reconnect as snapshot-first
- `turn/attach` is daemon-instance scoped rather than a claim of eternal turn continuity

This keeps the supervision model predictable: Orcas owns the local truth, Codex executes work, and the operator surfaces read from the daemon rather than reconstructing state from raw execution chatter.
