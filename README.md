![ORCAS Logo](assets/orcas_banner.png)

# Orcas

_Open Reasoning Context & Agent Supervisor_  

Orcas is a local Rust supervisor layer built around [`codex`](https://github.com/openai/codex) [`app-server`](https://developers.openai.com/codex/app-server/).

Orcas is for the point where one agent thread is no longer enough. It keeps the control plane close: local, durable, inspectable, and calm. `orcasd` owns workflow state, lifecycle, local IPC, snapshots, and event streams. The CLI (`orcas`) is a client of that daemon.

Codex remains the execution substrate. Orcas keeps the shape of the work around that execution: workstreams, work units, assignments, threads, turns, reports, and supervisor decisions. That separation matters. It means the state that matters does not vanish into terminal scrollback, and it means review stays human. Supervisor proposals are artifacts for inspection, not hidden authority.

## Why Orcas

Orcas is useful when the work branches. Maybe one repository becomes three. Maybe one task opens into an implementation lane, a review lane, and a cleanup lane. Maybe you want several agents moving at once across separate Git worktrees or entirely separate codebases, but you still want one readable picture of what is active, what is waiting, and what needs a decision.

The model stays simple once it becomes familiar. A workstream holds the larger objective. Inside it, work units describe concrete pieces of work. Assignments connect that work to an execution session. Threads and turns give you the Codex-side view. The supervisor layer sits above that flow and gives you a place to review, steer, interrupt, continue, or close the loop without treating the worker transcript itself as the source of truth.

What Orcas offers is not more automation for its own sake. It offers calmer automation: a way to let multiple agent threads move quickly without losing the sense of where the work is, why it exists, and what should happen next.

## Usage

A common pattern is to open a new workstream for an objective, create one or more threads beneath it, and let those threads map cleanly to separate worktrees or separate repositories. From there, the supervisor can inspect the live state from the CLI. That is where Orcas feels especially strong: several active threads, several possible next actions, and a local view that stays coherent as the system moves.

The CLI remains close at hand for scripted flows, quick checks, and direct operator actions, including authored steer creation, replacement, review, approve/send, reject, record-no-action, manual-refresh, and a cross-thread Codex decision queue/history surface for supervised threads.

## Current Operator Surface

The strongest checked-in operator surfaces today are the CLI and the daemon IPC contract.

- `orcasd` is the durable local control plane
- `orcas` is the primary checked-in operator client
- the repo does not currently contain a primary UI surface to "resume"

That matters for planning work. Orcas is currently best understood and operated through CLI flows, daemon-backed state inspection, and the checked-in integration/E2E harnesses rather than through a separate frontend.

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

Once the daemon is running, `orcas doctor` is the quickest way to confirm that Orcas can see its configuration, runtime paths, socket, and Codex endpoint. From there, you can stay in the CLI.

## Testing

The fast developer path stays the same:

- `make test` runs the normal Rust test suite
- `cargo test` continues to behave like standard Rust testing

End-to-end operator workflows are available as an opt-in lane under `tests/e2e/`:

- `make test-e2e`
- `make test-e2e-live`
- `make test-e2e-long`
- `make test-e2e SCENARIO=<name>`
- `make clean-e2e`

Generated E2E output is kept under `target/e2e/` so it is easy to inspect and easy to remove.

Current harness contract:

- `make test-e2e` is the daily deterministic confidence lane and should work from a normal dirty checkout
- scenarios that require a clean git tree are opt-in and must not be default-enabled
- the default deterministic lane remains model-free
- proposal-bearing live supervisor scenarios may use an explicit local OpenAI-compatible endpoint, but that support is test scaffolding only and not a default product dependency

Recent progress that is now on `main`:

- recover malformed live worker report envelopes to supervisor-reviewable `Ambiguous` state instead of hard-failing every damaged envelope
- preserve assignment communication context across turn ingestion so redirected or successor assignments keep the intended execution `cwd`
- restore a trustworthy dirty-checkout deterministic E2E lane

## Implementation

Orcas is written in Rust and designed to be fast and portable. The runtime is built on [Tokio](https://tokio.rs/), which keeps the daemon responsive under concurrent work. The daemon talks to local clients over a Unix domain socket, keeps snapshots and event streams close to the machine, and avoids turning the control plane into a heavyweight web service when it does not need to be one.

Inside the workspace, the responsibilities are separated cleanly. `orcas-core` holds shared types, errors, paths, and IPC structures. `orcas-codex` handles the Codex connection and typed `app-server` surface. `orcasd` builds `orcasd`, the long-lived daemon. `orcas` builds the `orcas` CLI.

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

Orcas follows the XDG layout on Linux. The user configuration file lives at `~/.config/orcas/config.toml`. State, logs, and runtime files live under the same user-scoped XDG directories for the CLI and the packaged systemd user service.

```text
config:  ~/.config/orcas/config.toml
state:   ~/.local/share/orcas/state.json
db:      ~/.local/share/orcas/state.db
logs:    ~/.local/share/orcas/logs/
socket:  ${XDG_RUNTIME_DIR:-~/.local/share/orcas/runtime}/orcas/orcasd.sock
meta:    ${XDG_RUNTIME_DIR:-~/.local/share/orcas/runtime}/orcas/orcasd.json
```

`state.json` remains the live collaboration and thread/turn mirror store. `state.db` is the live authority store for authority workstreams, authority work units, and tracked threads.

The packaged `orcas-daemon.service` unit is a user service, not a root-owned global daemon. It is intended to run under `systemctl --user` so the daemon and CLI resolve the same XDG config, data, log, and socket paths.

For source installs, `make install-systemd` writes that user unit into your user manager directory and rewrites `ExecStart` to the current install prefix so it follows the binary path you actually chose.

The current read model is split:

- `state/get` is a merged daemon snapshot that includes collaboration state plus any explicit authority compatibility bridge summaries needed for assignment execution
- `authority/hierarchy/get` is the canonical authority-only hierarchy query for planning hierarchy, tracked threads, revisions, and other authority metadata; other clients should prefer authority reads when they need canonical planning state

Recovery is snapshot-first rather than replay-based:

- if a daemon connection drops or the daemon restarts, clients reconnect, reload current reads, and then resubscribe for new events
- old event subscriptions are tied to the old socket lifetime and are not a missed-history replay channel
- clients reload both `state/get` and `authority/hierarchy/get` after reconnect, then refetch focused authority detail when the selected row still exists

The current operator mutation surface is also split, but no longer ambiguous:

- `orcas workstreams ...`, `orcas workunits ...`, and `orcas tracked-threads ...` are the canonical authority-backed planning hierarchy CRUD commands
- `workunit/get` remains a daemon runtime-detail exception for collaboration execution detail; it is not a canonical planning API
- there is no longer an operator-facing legacy planning command namespace; retained collaboration planning state is now an internal compatibility concern rather than a peer CLI surface
- assignment, report, decision, proposal, thread, and turn flows remain collaboration- or runtime-oriented daemon surfaces rather than authority planning CRUD

`RUST_LOG` controls tracing verbosity. Orcas-specific overrides use `ORCAS_*` environment variables, including the Codex binary path and upstream listen URL.

Two runtime-mode overrides are worth calling out explicitly:

- `ORCAS_CONNECTION_MODE=connect_only` forces connect-only mode
- `ORCAS_CONNECTION_MODE=spawn_always` forces spawn mode

If `ORCAS_CONNECTION_MODE` is unset, Orcas keeps the configured or default `spawn_if_needed` behavior. The CLI and daemon flags `--connect-only` and `--force-spawn` are mutually exclusive one-shot overrides for the same setting.

## Branch And Worktree Hygiene

Use bounded integration branches when merging validated repair stacks back toward `main`.

- keep active dirty lanes intentionally, not accidentally
- remove temporary integration worktrees after the validated merge lands
- delete local branches only when they are fully merged or otherwise clearly superseded
- preserve unmerged or dirty debug branches until their state is intentionally archived or discarded

## Read more

For a fuller technical picture, see [Architecture](docs/architecture.md), [Collaboration](docs/collaboration.md), [Local-Authority MVP Backend Design](docs/design/local-authority-mvp-backend.md), [Installation](docs/install.md), [Configuration](docs/configuration.md), [Logging](docs/logging.md), [Operations](docs/operations.md), and [Testing](docs/testing.md).

## License

Licensed under Apache 2.0. See [LICENSE](LICENSE).
