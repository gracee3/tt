# Orcas Architecture

## Overview

Orcas is a local-first orchestration system for supervising agent workflows on a single machine. The daemon is the authority for workflow state, lifecycle transitions, and local IPC. The CLI is a client of that daemon. The TUI also reads and mutates supervised state through the daemon, but it has one intentional local exception: it can launch a PTY-backed `codex resume` session for interactive attachment to a selected thread.

The control plane stays local. Orcas owns the records that matter for supervision, while Codex remains the execution substrate underneath those records. That separation keeps workflow state explicit and inspectable, and it gives the daemon a single place to coordinate startup, persistence, and event delivery.

## Runtime Roles

`orcas` is the operator-facing CLI. It is used for daemon lifecycle commands, status inspection, workflow review, and other supervisor actions that need to go through the daemon. For planning hierarchy CRUD, the CLI now converges on the same authority surface the TUI already uses.

`orcasd` is the long-lived service process. On startup it resolves configuration, ensures the runtime and data directories exist, writes runtime metadata, binds the local socket, and connects to the upstream Codex app-server. From that point on it serves local clients and owns the live in-memory view of Orcas state.

`orcas-tui` is the interactive terminal client. It renders the same daemon state as the CLI, but presents it in a live UI for inspection and control. Its supervised-state reads and writes go through the daemon over local IPC. Separately, it can launch a local PTY-backed `codex resume` helper for interactive attachment to a selected thread.

## State and Communication

Orcas uses a local Unix domain socket for IPC. The wire format is structured JSON-RPC 2.0, exchanged as line-delimited JSON messages. Clients use requests for commands and queries, responses for returned data, and notifications for state-change events.

The daemon provides both snapshots and events. A client can ask for a point-in-time snapshot to bootstrap its view, then subscribe to events to keep that view current. The CLI relies on focused RPCs for authority-backed planning CRUD and on `state/get` plus focused RPCs for collaboration and runtime views. The TUI bootstraps from both `state/get` and `authority/hierarchy/get`, because `state/get` is now a collaboration-first snapshot plus any explicit assignment-compatibility bridge rows rather than a general authority planning read. Authority workstream, work unit, and tracked-thread CRUD mutations emit post-commit lifecycle notifications, but those notifications are still visibility signals layered on top of authority reloads rather than a replacement for canonical authority reads.

Recovery remains snapshot-first rather than replay-based. If a daemon connection drops or the daemon restarts, old event subscriptions close with that socket lifetime. Clients reconnect, reload current state, and then establish a fresh subscription; they do not assume missed history will be replayed.

The daemon’s state model is Orcas-native, but it currently has two live local persistence systems. Legacy collaboration and thread/turn mirror state are loaded from and persisted to `state.json`. Authority-owned workstreams, work units, and tracked threads are stored in `state.db` with explicit commands, revisions, and tombstones. `state/get` is therefore a merged derived snapshot rather than a single-store read, while `authority/hierarchy/get` and authority detail RPCs remain the canonical planning hierarchy surfaces.

## Workflow Lifecycle

Workstreams group related work under a shared objective. Work units break that objective into concrete tasks. Assignments bind a worker session to a work unit and define the instructions and status of that execution.

Thread and turn state describe the Codex-side execution view that Orcas supervises. A thread may be started or resumed, attached or detached, and observed through turn history and live events. Turns may be steered, interrupted, or allowed to complete. The daemon keeps enough state to answer what is active, what is terminal, and what is only queryable as historical data.

Reports and decisions close the loop. Worker reports and supervisor decisions are recorded back into Orcas state, and the daemon emits lifecycle events so the CLI and TUI can react without reconstructing history from raw upstream traffic.

## Execution Model

The daemon owns the upstream Codex connection and the local supervision state. Supervisors do not own either one. They connect to `orcasd` on demand, ask for state, and issue commands through the daemon’s API surface.

If the daemon is managed by systemd, the current packaged unit is intended to run under the user manager so it shares the same XDG paths as the CLI and TUI. If it is run manually, it behaves like a normal foreground process. In both cases the daemon is the long-lived process and the clients are transient.

This separation matters operationally. Restarting the CLI does not affect the Codex connection. Restarting the daemon does, because it owns the live upstream session and the supervised state surfaces. Restarting the TUI does not affect daemon-owned state, but any TUI-local PTY-backed `codex resume` session belongs to that TUI process rather than the daemon. After a daemon restart, the TUI invalidates its authority-only hierarchy and detail caches, reloads `state/get`, reloads `authority/hierarchy/get`, and then refetches focused detail when the selected row still exists. If the TUI process itself remains alive, an already-running local PTY session can continue independently during daemon reconnect because it is not daemon-managed state.

## Design Principles

Orcas follows a small set of consistent rules:

1. Local-first: state, runtime metadata, and IPC stay on the host.
2. Separation of control and execution: the supervisor controls, the daemon orchestrates, and Codex executes.
3. Deterministic state where possible: workflow records are explicit and persisted rather than inferred from UI state.
4. Inspectability: snapshots, events, and runtime metadata are available to clients instead of being hidden inside a transcript.
5. Minimal external surface: the daemon listens on a local socket rather than a public network port.

## Current Boundary

The SQLite-backed local-authority model is already live for authority workstreams, authority work units, and tracked threads. It runs alongside the legacy collaboration store rather than replacing it completely today. The current daemon therefore has a real boundary between:

1. collaboration-owned state persisted in `state.json`
2. authority-owned state persisted in `state.db`
3. merged read models such as `state/get`
4. client-side derived state inside the TUI

That boundary is the focus of the current hardening work. The detailed current-state contract lives in [Collaboration](collaboration.md). The original local-authority design rationale still lives in [Local-Authority MVP Backend Design](design/local-authority-mvp-backend.md), and tracked-thread semantics remain defined by [ADR 0001](adr/0001-tracked-thread-is-a-local-binding-record.md).
