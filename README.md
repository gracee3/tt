![ORCAS Logo](orcas_banner.png)

# Orcas

_Open Reasoning Context & Agent Supervisor_  
Tracking Codex `v0.115.0`. Reference upstream: <https://github.com/openai/codex>

Orcas is a local Rust supervisor layer built around Codex `app-server`.

`orcasd` owns the upstream Codex connection, Orcas-owned persistent workflow state, local IPC, live snapshots, and event streams. The CLI (`orcas`) and TUI (`orcas-tui`) are clients of that daemon rather than direct Codex clients.

Orcas state is authoritative. Codex remains the worker execution substrate. Supervisor proposals are review artifacts, not workflow truth, and human approval is required before Orcas records an authoritative `Decision` or creates a successor `Assignment`.

## App Summary

- local daemon: `orcasd` manages one long-lived Codex WebSocket connection and a Unix domain socket for local clients
- control plane: Orcas persists workstreams, work units, assignments, worker sessions, reports, decisions, and supervisor proposals
- operator surfaces: `orcas` exposes CLI-first supervisor workflows and `orcas-tui` provides read-only collaboration/state inspection
- supervisor reasoning: Orcas builds explicit context packs from canonical state, calls a Responses-backed reasoner, validates the result, persists the proposal, and requires human approval before any authoritative state change
- observability: daemon snapshots, lifecycle events, and on-demand getters expose the current workflow state without relying on hidden transcript state

## Workspace

- `crates/orcas-core`: shared config, errors, paths, events, IPC types, session store
- `crates/orcas-codex`: Codex WebSocket client, daemon launch, typed app-server slice
- `crates/orcas-daemon`: `orcasd`, UDS server, event fanout, shared IPC client/process manager
- `crates/orcas-supervisor`: `orcas` CLI that talks to `orcasd`
- `crates/orcas-tui`: TUI app core, runtime, render layer, and tests

## Build

```bash
cd /home/emmy/git/orcas
cargo fmt
cargo check
cargo test
```

## Runtime Files

- config: `~/.config/orcas/config.toml`
- state: `~/.local/share/orcas/state.json`
- daemon log: `~/.local/share/orcas/logs/orcasd.log`
- Codex app-server log: `~/.local/share/orcas/logs/codex-app-server.log`
- runtime socket: `${XDG_RUNTIME_DIR:-~/.local/share/orcas/runtime}/orcas/orcasd.sock`
- daemon runtime metadata: `${XDG_RUNTIME_DIR:-~/.local/share/orcas/runtime}/orcas/orcasd.json`

## Codex Binary

Default Orcas config points at:

- `/home/emmy/git/codex/codex-rs/target/debug/codex`

Build it if needed:

```bash
cd /home/emmy/git/codex/codex-rs
cargo build -p codex-cli --bin codex
```

## Optional Auto Proposal Trigger

Automatic supervisor proposal creation on `report_recorded` is available, but it is opt-in and disabled by default.

Enable it in `~/.config/orcas/config.toml`:

```toml
[supervisor.proposals]
auto_create_on_report_recorded = true
```

When enabled, `orcasd` automatically creates a reviewable supervisor proposal for an eligible authoritative worker report.

This does not:

- auto-approve a proposal
- create authoritative decisions by itself
- dispatch the next assignment by itself
- enable scheduling, planning, or swarm behavior
- relax strict same-report suppression for repeated proposal generation
