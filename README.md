![ORCAS Logo](assets/orcas_banner.png)

# Orcas

_Open Reasoning Context & Agent Supervisor_  

Orcas is a local Rust supervisor layer built around [`codex`](https://github.com/openai/codex) [`app-server`](https://developers.openai.com/codex/app-server/).

Orcas is for the point where one agent thread is no longer enough. It keeps the control plane close: local, durable, inspectable, and calm. `orcasd` owns workflow state, lifecycle, local IPC, snapshots, and event streams. The CLI (`orcas`) is a client of that daemon. The TUI (`orcas-tui`, also available as `orcas tui`) also reads and mutates supervised state through the daemon, while keeping one local exception for PTY-backed `codex resume` sessions.

Codex remains the execution substrate. Orcas keeps the shape of the work around that execution: workstreams, work units, assignments, threads, turns, reports, and supervisor decisions. That separation matters. It means the state that matters does not vanish into terminal scrollback, and it means review stays human. Supervisor proposals are artifacts for inspection, not hidden authority.

## Why Orcas

Orcas is useful when the work branches. Maybe one repository becomes three. Maybe one task opens into an implementation lane, a review lane, and a cleanup lane. Maybe you want several agents moving at once across separate Git worktrees or entirely separate codebases, but you still want one readable picture of what is active, what is waiting, and what needs a decision.

The model stays simple once it becomes familiar. A workstream holds the larger objective. Inside it, work units describe concrete pieces of work. Assignments connect that work to an execution session. Threads and turns give you the Codex-side view. The supervisor layer sits above that flow and gives you a place to review, steer, interrupt, continue, or close the loop without treating the worker transcript itself as the source of truth.

What Orcas offers is not more automation for its own sake. It offers calmer automation: a way to let multiple agent threads move quickly without losing the sense of where the work is, why it exists, and what should happen next.

## Usage

A common pattern is to open a new workstream for an objective, create one or more threads beneath it, and let those threads map cleanly to separate worktrees or separate repositories. From there, the supervisor can inspect the live state from the CLI or move into the TUI when the work becomes more visual and parallel. That is where Orcas feels especially strong: several active threads, several possible next actions, and a local view that stays coherent as the system moves.

The TUI is not only a dashboard. It is part of the control surface. It makes room for collaboration, turn review, next-turn approval, deliberate no-action recording, manual refresh of idle-thread proposals, multiline steer authoring, interruption, and state inspection in a form that stays fast and readable even when the work is spread across multiple threads. Steer revisions remain visible in thread-local history instead of disappearing behind the latest pending decision. The CLI remains close at hand for scripted flows, quick checks, and direct operator actions, including authored steer creation, replacement, review, approve/send, reject, record-no-action, manual-refresh, and a cross-thread Codex decision queue/history surface for supervised threads.

## Quick start

On Linux, the easiest install path is a `.deb` package. If you are working from a release archive, the tarball layout is equally simple.

```bash
sudo dpkg -i ./orcas_0.1.0_amd64.deb
systemctl --user enable --now orcas-daemon.service
orcas doctor
```

Or, from a tarball release:

```bash
tar -xzf orcas-v0.1.0-x86_64-unknown-linux-gnu.tar.gz
cd orcas-v0.1.0-x86_64-unknown-linux-gnu
./bin/orcas doctor
./bin/orcasd
```

Once the daemon is running, `orcas doctor` is the quickest way to confirm that Orcas can see its configuration, runtime paths, socket, and Codex endpoint. From there, you can stay in the CLI or open the TUI.

```bash
orcas-tui
```

## Implementation

Orcas is written in Rust and designed to be fast and portable. The runtime is built on [Tokio](https://tokio.rs/), which keeps the daemon responsive under concurrent work, and the terminal interface is built with [Ratatui](https://ratatui.rs/), which keeps the interactive surface fast, lightweight, and comfortable to live in. The daemon talks to local clients over a Unix domain socket, keeps snapshots and event streams close to the machine, and avoids turning the control plane into a heavyweight web service when it does not need to be one.

Inside the workspace, the responsibilities are separated cleanly. `orcas-core` holds shared types, errors, paths, and IPC structures. `orcas-codex` handles the Codex connection and typed `app-server` surface. `orcasd` builds `orcasd`, the long-lived daemon. `orcas` builds the `orcas` CLI. `orcas-tui` provides the interactive terminal client.

## Building from source

If you are building from source, install a stable Rust toolchain and build from the repository root.

```bash
cargo fmt
cargo check
cargo test
make build
```

Orcas expects a local Codex binary. The development default may point at a source-tree build, but in normal use you should set the installed path in configuration or with `ORCAS_CODEX_BIN`. A typical local build of Codex looks like this:

```bash
cd /path/to/codex/codex-rs
cargo build -p codex-cli --bin codex
```

## Paths and configuration

Orcas follows the XDG layout on Linux. The user configuration file lives at `~/.config/orcas/config.toml`. State, logs, and runtime files live under the same user-scoped XDG directories for the CLI, the TUI, and the packaged systemd user service.

```text
config:  ~/.config/orcas/config.toml
state:   ~/.local/share/orcas/state.json
db:      ~/.local/share/orcas/state.db
logs:    ~/.local/share/orcas/logs/
socket:  ${XDG_RUNTIME_DIR:-~/.local/share/orcas/runtime}/orcas/orcasd.sock
meta:    ${XDG_RUNTIME_DIR:-~/.local/share/orcas/runtime}/orcas/orcasd.json
```

`state.json` remains the live collaboration and thread/turn mirror store. `state.db` is the live authority store for authority workstreams, authority work units, and tracked threads.

The packaged `orcas-daemon.service` unit is a user service, not a root-owned global daemon. It is intended to run under `systemctl --user` so the daemon, CLI, and TUI all resolve the same XDG config, data, log, and socket paths.

For source installs, `make install-systemd` writes that user unit into your user manager directory and rewrites `ExecStart` to the current install prefix so it follows the binary path you actually chose.

The current read model is split:

- `state/get` is a merged daemon snapshot that includes collaboration state plus any explicit authority compatibility bridge summaries needed for assignment execution
- `authority/hierarchy/get` is the canonical authority-only hierarchy query for planning hierarchy, tracked threads, revisions, and other authority metadata; the TUI uses it directly and other clients should prefer authority reads when they need canonical planning state

Recovery is snapshot-first rather than replay-based:

- if a daemon connection drops or the daemon restarts, clients reconnect, reload current reads, and then resubscribe for new events
- old event subscriptions are tied to the old socket lifetime and are not a missed-history replay channel
- the TUI reloads both `state/get` and `authority/hierarchy/get` after reconnect, then refetches focused authority detail when the selected row still exists
- TUI-local PTY-backed `codex resume` sessions are not rebuilt by daemon reconnect or restart; if the TUI process stays alive, an already-running local PTY session remains a TUI-owned attachment surface rather than daemon-persisted state

The current operator mutation surface is also split, but no longer ambiguous:

- `orcas workstreams ...`, `orcas workunits ...`, and `orcas tracked-threads ...` are the canonical authority-backed planning hierarchy CRUD commands
- `orcas legacy-workstreams ...` and `orcas legacy-workunits ...` remain available as explicit create/list/get compatibility paths for legacy collaboration records
- assignment, report, decision, proposal, thread, and turn flows remain collaboration- or runtime-oriented daemon surfaces rather than authority planning CRUD

`RUST_LOG` controls tracing verbosity. Orcas-specific overrides use `ORCAS_*` environment variables, including the Codex binary path and upstream listen URL.

Two runtime-mode overrides are worth calling out explicitly:

- `ORCAS_CONNECTION_MODE=connect_only` forces connect-only mode
- `ORCAS_CONNECTION_MODE=spawn_always` forces spawn mode

If `ORCAS_CONNECTION_MODE` is unset, Orcas keeps the configured or default `spawn_if_needed` behavior. The CLI and daemon flags `--connect-only` and `--force-spawn` are mutually exclusive one-shot overrides for the same setting.

## Read more

For a fuller technical picture, see [Architecture](docs/architecture.md), [Collaboration](docs/collaboration.md), [Local-Authority MVP Backend Design](docs/design/local-authority-mvp-backend.md), [Installation](docs/install.md), [Configuration](docs/configuration.md), [Logging](docs/logging.md), [Operations](docs/operations.md), and [Testing](docs/testing.md).

## License

Licensed under Apache 2.0. See [LICENSE](LICENSE).
