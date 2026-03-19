# Orcas Architecture

## Overview

Orcas is a local-first orchestration system for supervising agent workflows on a single machine. The daemon is the authority for workflow state, lifecycle transitions, and local IPC. The CLI and TUI are clients of that daemon, not direct Codex clients.

The control plane stays local. Orcas owns the records that matter for supervision, while Codex remains the execution substrate underneath those records. That separation keeps workflow state explicit and inspectable, and it gives the daemon a single place to coordinate startup, persistence, and event delivery.

## Runtime Roles

`orcas` is the operator-facing CLI. It is used for daemon lifecycle commands, status inspection, workflow review, and other supervisor actions that need to go through the daemon.

`orcasd` is the long-lived service process. On startup it resolves configuration, ensures the runtime and data directories exist, writes runtime metadata, binds the local socket, and connects to the upstream Codex app-server. From that point on it serves local clients and owns the live in-memory view of Orcas state.

`orcas-tui` is the interactive terminal client. It renders the same daemon state as the CLI, but presents it in a live UI for inspection and control. It still talks to the daemon over the local IPC layer.

## State and Communication

Orcas uses a local Unix domain socket for IPC. The wire format is structured JSON-RPC 2.0, exchanged as line-delimited JSON messages. Clients use requests for commands and queries, responses for returned data, and notifications for state-change events.

The daemon provides both snapshots and events. A client can ask for a point-in-time snapshot to bootstrap its view, then subscribe to events to keep that view current. This is the basic pattern used by both the CLI and the TUI: read the current state first, then follow incremental changes.

The daemon’s state model is Orcas-native. It tracks workstreams, work units, assignments, worker sessions, reports, decisions, proposals, thread summaries, and turn summaries. That state is persisted locally and updated through explicit transitions rather than inferred from a transient terminal session.

## Workflow Lifecycle

Workstreams group related work under a shared objective. Work units break that objective into concrete tasks. Assignments bind a worker session to a work unit and define the instructions and status of that execution.

Thread and turn state describe the Codex-side execution view that Orcas supervises. A thread may be started or resumed, attached or detached, and observed through turn history and live events. Turns may be steered, interrupted, or allowed to complete. The daemon keeps enough state to answer what is active, what is terminal, and what is only queryable as historical data.

Reports and decisions close the loop. Worker reports and supervisor decisions are recorded back into Orcas state, and the daemon emits lifecycle events so the CLI and TUI can react without reconstructing history from raw upstream traffic.

## Execution Model

The daemon owns the upstream Codex connection and the local supervision state. Supervisors do not own either one. They connect to `orcasd` on demand, ask for state, and issue commands through the daemon’s API surface.

If the daemon is managed by systemd, the service is started and stopped there. If it is run manually, it behaves like a normal foreground process. In both cases the daemon is the long-lived process and the clients are transient.

This separation matters operationally. Restarting the CLI or TUI does not affect the Codex connection. Restarting the daemon does, because it owns the live upstream session and the authoritative local state.

## Design Principles

Orcas follows a small set of consistent rules:

1. Local-first: state, runtime metadata, and IPC stay on the host.
2. Separation of control and execution: the supervisor controls, the daemon orchestrates, and Codex executes.
3. Deterministic state where possible: workflow records are explicit and persisted rather than inferred from UI state.
4. Inspectability: snapshots, events, and runtime metadata are available to clients instead of being hidden inside a transcript.
5. Minimal external surface: the daemon listens on a local socket rather than a public network port.

## Current Direction

The next persistence pass moves Orcas from the current JSON snapshot store to a SQLite-backed local-authority model with explicit commands, canonical events, and read projections for the TUI. The design for that pass lives in [Local-Authority MVP Backend Design](design/local-authority-mvp-backend.md), with the tracked-thread semantics captured in [ADR 0001](adr/0001-tracked-thread-is-a-local-binding-record.md).
