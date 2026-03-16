# Orcas

Orcas is a local Rust orchestration scaffold for Codex `app-server`.

Current shape:

- `orcasd` owns one long-lived upstream Codex WebSocket connection
- local frontends attach to `orcasd` over a Unix domain socket
- Orcas keeps its own narrow IPC and protocol layers instead of depending on Codex workspace crates

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
- supervisor CLI routed through the daemon
- TUI placeholder routed through the daemon
- lightweight JSON/TOML config and thread metadata persistence

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
- `crates/orcas-tui`: placeholder TUI that talks to `orcasd`
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
- `models/list`
- `threads/list`
- `thread/start`
- `thread/read`
- `thread/resume`
- `turn/start`
- `turn/interrupt`
- `events/subscribe`

## Known Limitations

- Unix-only local IPC for now
- WebSocket-only upstream Codex transport for now
- one configured Codex backend only
- approvals are still rejected by default
- `threads list` still reflects the raw upstream thread set, which can be broader than Orcas-created threads
- TUI is intentionally minimal and read-mostly in this pass
- Orcas IPC is intentionally narrow and may evolve

## Validation Completed In This Pass

Validated locally against `/home/emmy/git/codex/codex-rs/target/debug/codex`:

- `cargo check`
- `cargo test`
- `orcas supervisor daemon start`
- `orcas supervisor models list`
- `orcas supervisor quickstart`
- TUI attached to the same live daemon
- supervisor-driven turn events observed from the TUI
- killing the TUI did not kill `orcasd`
- supervisor commands continued working after the TUI disconnected

## Next Step

Build a richer Orcas-owned state/query layer inside `orcasd` so clients can query live thread state, recent turns, and resumable session metadata without repeatedly round-tripping raw upstream lists.
