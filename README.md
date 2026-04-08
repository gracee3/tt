![TT Logo](assets/orcas_banner.png)

# TT

_Open Reasoning Context & Agent Supervisor_

TT is a local Rust supervisor layer built around Codex app-server.

## What This Repo Is

This repository is the TT v2 workbench.

The current direction is:

- `tt-daemon` as the local API boundary
- `tt-tui` as the primary operator surface
- `tt-cli` as a thin client over the daemon
- `.tt` as the TT-owned overlay for orchestration metadata
- `tt-git` as the repo/worktree authority for merge readiness and checkout inspection

The design stays local first. State lives on the machine, operators inspect it directly, and merge/cleanup decisions stay visible instead of hidden behind a remote service.

## Core Shape

TT is built for the point where one agent thread is no longer enough.

The system keeps the control plane close:

- durable local workflow state
- explicit workstreams, work units, assignments, threads, turns, reports, and supervisor decisions
- daemon-backed snapshots and event streams
- workspace and merge semantics that stay tied to the local checkout

That separation matters because it keeps the operator view coherent even when the work branches across multiple threads or worktrees.

## Current Focus

The v2 line is centered on these responsibilities:

- Codex remains the runtime/source of truth for threads, turns, rollouts, sandboxing, and app-server transport
- TT owns the overlay/orchestration layer around `.tt`, workspace bindings, and operator workflows
- `tt-daemon` exposes the local boundary and keeps runtime state in one place
- `tt-tui` is the main place to inspect and operate the system
- `tt-cli` stays narrow and delegates behavior to the daemon

The intent is not to build a remote platform first. The intent is to make the local supervisor stack simpler, clearer, and easier to operate.

## Build And Test

```bash
cargo fmt
cargo check
cargo test
make test
```

End-to-end operator workflows live under `tests/e2e/`:

- `make test-e2e`
- `make test-e2e-live`
- `make test-e2e-long`
- `make test-e2e SCENARIO=<name>`
- `make clean-e2e`

## Working Notes

- `make test` runs the standard Rust test suite.
- `make test-e2e` is the normal deterministic E2E lane.
- `TT_HOME` can point TT at an isolated config/state/log root.
- `state/get` is the merged daemon snapshot.
- `authority/hierarchy/get` is the canonical authority query for planning hierarchy and tracked-thread metadata.

## More Docs

- [TT v2 Architecture](docs/tt_v2_architecture.md)
- [Phase 2 Plan](docs/PHASE2_EXECUTION_PLAN.md)
- [Next Milestones](docs/NEXT_MILESTONES.md)
- [Workstream Todo](docs/WORKSTREAM_TODO.md)
- [Workspace Lifecycle](docs/workspace-lifecycle.md)

## License

Licensed under Apache 2.0. See [LICENSE](LICENSE).
