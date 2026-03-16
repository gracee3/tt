# Orcas

_Orchestrating Rust Codex App Server_  
Tracking Codex `v0.115.0`. Reference upstream: <https://github.com/openai/codex>

Orcas is a local Rust orchestration scaffold for Codex `app-server`.

Current milestone status: foundation complete and intentionally narrow. The current repo state is meant to be a stable pause point before broader product work changes direction.

Current shape:

- `orcasd` owns one long-lived upstream Codex WebSocket connection
- local frontends attach to `orcasd` over a Unix domain socket
- Orcas keeps its own narrow IPC and protocol layers instead of depending on Codex workspace crates
- `orcasd` now exposes a live snapshot/query layer for frontend bootstrap
- `orcasd` now writes runtime metadata next to the socket and reports build fingerprints for lifecycle debugging
- `orcasd` now supports graceful shutdown over IPC
- `orcas-tui` is now driven by a canonical reducer/runtime with headless tests
- `orcas-tui` now reconnects automatically with snapshot-first rebootstrap after daemon replacement

This pass establishes the broker/service boundary. It is not the full product yet.

## Why This Topology

Codex is treated as an external engine process.

Orcas intentionally targets:

- upstream: `codex app-server --listen ws://127.0.0.1:PORT`
- local IPC: Unix domain socket
- semantics: narrow JSON-RPC-style request/response/notification flow

That gives Orcas the right shape for:

- a background local daemon
- multiple independent clients
- future richer TUI work
- future browser/backend attachment

## Current Scope

Implemented now:

- Rust workspace with separate core, Codex integration, daemon, supervisor, and TUI crates
- persistent `orcasd` process
- Orcas-owned UDS IPC contract
- one persistent Codex WebSocket client inside the daemon
- event subscription and fanout to multiple local clients
- Orcas-owned live state snapshot/query surface for frontends
- version-aware daemon status and explicit restart support
- graceful daemon stop over IPC
- runtime metadata file next to the socket for PID/build inspection
- supervisor CLI routed through the daemon
- supervisor turn inspection commands for active turn/state visibility
- supervisor prompt and quickstart flows now use a session-aware streaming helper with bounded reconnect and snapshot-first recovery
- daemon-owned active-turn registry with explicit `turn/get`, `turn/attach`, and `turns/list_active` Orcas IPC methods
- TUI reducer/runtime/view-model split routed through the daemon with automatic reconnect/resubscribe and turn-level status consumption
- headless TUI test harness with fake backend
- lightweight JSON/TOML config and thread metadata persistence, including recent output snippets

Not implemented yet:

- browser UI
- approval UX
- multiple upstream Codex backends
- rollback/fork/review flows
- stdio transport fallback

## Workspace

- `crates/orcas-core`: shared config, errors, paths, events, IPC types, session store
- `crates/orcas-codex`: Codex WebSocket client, daemon launch, typed app-server slice
- `crates/orcas-daemon`: `orcasd`, UDS server, event fanout, shared IPC client/process manager
- `crates/orcas-supervisor`: `orcas` CLI that talks to `orcasd`
- `crates/orcas-tui`: real TUI app core, runtime, render layer, and headless tests
- `docs/architecture.md`: crate boundaries and lifecycle model
- `docs/orcas-daemon.md`: daemon behavior and runtime notes
- `docs/ipc-protocol.md`: Orcas IPC method/event surface
- `docs/codex-app-server-notes.md`: notes from the local Codex checkout

## Pinned Local Codex Binary

Default Orcas config pins:

- `/home/emmy/git/codex/codex-rs/target/debug/codex`

Build it if needed:

```bash
cd /home/emmy/git/codex/codex-rs
cargo build -p codex-cli --bin codex
```

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

## Proof Of Life

Development path:

```bash
cd /home/emmy/git/orcas
cargo build --workspace
```

Start or reuse the Orcas daemon:

```bash
cargo run -p orcas-supervisor -- supervisor doctor
cargo run -p orcas-supervisor -- supervisor daemon start
cargo run -p orcas-supervisor -- supervisor daemon status
cargo run -p orcas-supervisor -- supervisor daemon restart
cargo run -p orcas-supervisor -- supervisor daemon stop
```

List models through `orcasd`:

```bash
cargo run -p orcas-supervisor -- supervisor models list
```

Start a thread and send one prompt:

```bash
cargo run -p orcas-supervisor -- supervisor quickstart \
  --cwd /home/emmy/git/orcas \
  --model gpt-5.4 \
  --text "Say hello in one sentence."
```

Resume a thread and send another turn:

```bash
cargo run -p orcas-supervisor -- supervisor prompt \
  --thread <THREAD_ID> \
  --text "Summarize this repo in two bullets."
```

Attach the TUI to the same daemon:

```bash
cargo run -p orcas-tui
```

Direct daemon run for debugging:

```bash
cd /home/emmy/git/orcas
target/debug/orcasd
```

## Narrow Protocol Surface

Codex methods currently wrapped:

- `initialize`
- `thread/start`
- `thread/resume`
- `thread/read`
- `thread/list`
- `turn/start`
- `turn/interrupt`
- `model/list`

Codex notifications currently mapped:

- `thread/started`
- `thread/status/changed`
- `turn/started`
- `turn/completed`
- `item/started`
- `item/completed`
- `item/agentMessage/delta`

Orcas IPC currently exposes:

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
- `turns/list_active`
- `thread/resume`
- `turns/recent`
- `turn/get`
- `turn/attach`
- `turn/start`
- `turn/interrupt`
- `events/subscribe`

Supervisor operator/debug surfaces now include:

- `orcas supervisor turns list-active`
- `orcas supervisor turns get --thread <THREAD_ID> --turn <TURN_ID>`

IPC event notifications are now Orcas-owned daemon events rather than raw upstream Codex events. The current frontend-facing event slice includes:

- upstream status changes
- session/active turn changes
- thread summary updates
- turn updates
- item updates
- streamed output deltas
- warnings

Turn attachment semantics are intentionally conservative:

- live attachment is daemon-instance scoped
- `turn/attach` succeeds only when the current `orcasd` instance can still prove that the turn is live and attachable
- after daemon replacement, Orcas may still return terminal or cached turn state via `turn/get` or `turn/attach`, but it does not claim uninterrupted upstream execution unless continuity is still provable

## Known Limitations

- Unix-only local IPC for now
- WebSocket-only upstream Codex transport for now
- one configured Codex backend only
- approvals are still rejected by default
- `threads/list` is still broad, although `threads/list_scoped` and `state/get` now prefer Orcas-relevant threads
- TUI is intentionally minimal; the architecture and tests matter more than UI breadth in this pass
- Orcas IPC is intentionally narrow and may evolve
- supervisor one-shot commands still use short-lived retry behavior rather than long-lived session management
- supervisor streaming continuity is intentionally honest: Orcas can recover the local session view after daemon replacement, but it does not claim uninterrupted upstream Codex execution if the live turn was cut
- live turn attachment is currently daemon-instance scoped rather than durable across daemon replacement

## Validation Completed In This Pass

Validated locally against `/home/emmy/git/codex/codex-rs/target/debug/codex`:

- `cargo fmt --check`
- `cargo check`
- `cargo test`
- focused supervisor streaming-helper tests for reconnect, resubscribe, and honest interruption reporting
- daemon turn-registry and attachment-semantics tests
- supervisor turn inspection commands
- TUI turn-level status projection and active-turn tests
- `orcas supervisor daemon start`
- `orcas supervisor daemon status`
- `orcas supervisor daemon restart`
- `orcas supervisor daemon stop`
- `orcas supervisor models list`
- `orcas supervisor threads list`
- direct `state/get` over the UDS socket
- graceful `daemon/stop` IPC via a real `orcasd` process and cleanup of socket/metadata artifacts
- TUI left running across `daemon stop` and `daemon start`, then exited cleanly after the daemon returned
- TUI attached to the same live daemon and exited cleanly
- restarting a legacy daemon without runtime metadata and replacing it with the current build
- killing the daemon to leave a stale socket/metadata pair, then recovering with `supervisor daemon start`
- killing the TUI did not kill `orcasd`
- supervisor commands continued working after the TUI disconnected
- a live `supervisor prompt` run survived daemon replacement far enough to report bounded retry during `thread_resume` instead of failing immediately with a raw transport error
- supervisor streaming recovery is validated primarily through controlled fake-daemon tests because end-to-end upstream turn completion remains gated on Codex availability

Note: the previous quickstart path is currently blocked by the local Codex upstream returning `426 Upgrade Required` from its remote responses websocket. The Orcas daemon/TUI path validates cleanly, but end-to-end turn completion still depends on upstream Codex availability.

## Next Step

The foundation milestone is in a good stopping state. The next step should be a deliberate product-direction pivot rather than more incremental cleanup on this scaffold.
