# Orcas

_Orchestrating Rust Codex App Server_  
Tracking Codex `v0.115.0`. Reference upstream: <https://github.com/openai/codex>

Orcas is a local Rust orchestration scaffold for Codex `app-server`.

Current shape:

- `orcasd` owns one long-lived upstream Codex WebSocket connection
- local frontends attach to `orcasd` over a Unix domain socket
- Orcas keeps its own narrow IPC and protocol layers instead of depending on Codex workspace crates
- `orcasd` now exposes a live snapshot/query layer for frontend bootstrap
- `orcasd` now writes runtime metadata next to the socket and reports build fingerprints for lifecycle debugging
- `orcas-tui` is now driven by a canonical reducer/runtime with headless tests

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
- runtime metadata file next to the socket for PID/build inspection
- supervisor CLI routed through the daemon
- TUI reducer/runtime/view-model split routed through the daemon
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

IPC event notifications are now Orcas-owned daemon events rather than raw upstream Codex events. The current frontend-facing event slice includes:

- upstream status changes
- session/active turn changes
- thread summary updates
- turn updates
- item updates
- streamed output deltas
- warnings

## Known Limitations

- Unix-only local IPC for now
- WebSocket-only upstream Codex transport for now
- one configured Codex backend only
- approvals are still rejected by default
- `threads/list` is still broad, although `threads/list_scoped` and `state/get` now prefer Orcas-relevant threads
- TUI is intentionally minimal; the architecture and tests matter more than UI breadth in this pass
- Orcas IPC is intentionally narrow and may evolve
- TUI reconnect is refresh-driven after daemon restarts; it surfaces disconnect state cleanly but does not yet do a full automatic resubscribe loop in the background

## Validation Completed In This Pass

Validated locally against `/home/emmy/git/codex/codex-rs/target/debug/codex`:

- `cargo fmt --check`
- `cargo check`
- `cargo test`
- `orcas supervisor daemon start`
- `orcas supervisor daemon status`
- `orcas supervisor daemon restart`
- `orcas supervisor models list`
- `orcas supervisor threads list`
- direct `state/get` over the UDS socket
- TUI attached to the same live daemon and exited cleanly
- restarting a legacy daemon without runtime metadata and replacing it with the current build
- killing the daemon to leave a stale socket/metadata pair, then recovering with `supervisor daemon start`
- killing the TUI did not kill `orcasd`
- supervisor commands continued working after the TUI disconnected

Note: the previous quickstart path is currently blocked by the local Codex upstream returning `426 Upgrade Required` from its remote responses websocket. The Orcas daemon/TUI path validates cleanly, but end-to-end turn completion still depends on upstream Codex availability.

## Next Step

Broaden the Orcas-owned control plane beyond lifecycle hardening into richer daemon-side session workflows: tighter thread scoping, better active-turn recovery, and more explicit frontend reconnect/resubscribe behavior.
