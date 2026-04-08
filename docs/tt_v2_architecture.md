# TT v2 Architecture

## Summary

TT v2 is a Codex-first orchestration layer.

Codex remains the canonical runtime for:

- threads and turns
- rollouts and state indexing
- app-server transport and protocol
- sandbox semantics and command execution
- `.codex` session discovery

TT v2 adds a `.tt` overlay for:

- projects and work units
- bindings from work units to Codex threads
- workspace and worktree policy
- landing and merge orchestration
- review, todo, test, and integrate workflow overlays
- operator-facing query models and dashboards

The initial v2 delivery is local-only, TUI-first, and explicitly excludes `operator-web` and full remote/operator-server parity.

## Product Decisions

- Runtime model: Codex-first
- State model: `.tt` overlay only
- Initial UI: TUI first
- Migration style: parallel v2 stack
- Remote scope: deferred from the initial milestone
- CLI compatibility: clean break, except for minimal local bootstrapping aliases if implementation requires them

## Ownership Boundaries

### Codex owns

- thread lifecycle
- turn lifecycle
- rollout persistence and indexing
- app-server bindings
- sandbox policy and execution semantics
- canonical session and thread metadata in `.codex`

### TT owns

- project and work-unit structure
- workspace bindings to repos, branches, and worktrees
- merge readiness and landing policy
- operator workflow overlays
- daemon reconciliation of TT metadata against Codex and git state
- operator UI query models

### TT must not own

- a duplicate thread execution runtime
- a duplicate app-server protocol
- a duplicate transcript store
- a second source of truth for turn history

## Crate Layout

TT v2 should be implemented as a new parallel stack with the following crates.

### `tt-domain`

Pure TT domain types and policies.

Owns:

- projects
- work units
- thread bindings
- workspace bindings
- landing policy
- workflow overlays such as review, todo, test, and integrate

Must not depend on:

- sqlite
- clap
- tui or web frameworks
- Codex transport clients

### `tt-store`

SQLite-backed `.tt` overlay storage.

Owns:

- TT-only metadata
- bindings to Codex thread ids
- cached reconciliation checkpoints

Must not store:

- full thread transcripts
- turn/item history
- sandbox execution truth

### `tt-codex`

Adapter layer over Codex crates and `.codex`.

Owns:

- `.codex` discovery
- Codex state access
- thread list/read/start/resume wrappers
- app-server coordination hooks used by TT

### `tt-git`

Git and worktree orchestration.

Owns:

- repository discovery
- worktree lifecycle
- branch/base/head tracking
- merge readiness assessment
- landing execution helpers
- auto-merge hooks

### `tt-daemon`

Local orchestration and IPC only.

Owns:

- local IPC surface
- reconciliation between `.tt`, `.codex`, and git state
- background indexing and state refresh

Must stay small. It should coordinate services, not absorb policy, storage, protocol, and UI logic into one crate.

### `tt-ui-core`

Shared serializable query and view-model layer for the operator UI.

Owns:

- dashboard query models
- detail-page DTOs
- serde-friendly UI-facing state

This crate is the seam between the daemon and both frontends.

### `tt-tui`

Initial operator UI built on `tt-ui-core`.

### `tt-cli`

Thin CLI over daemon APIs.

## Dependency Rules

- `tt-domain` is the root TT crate and must remain pure.
- `tt-store` depends on `tt-domain`, not on UI crates.
- `tt-codex` depends on Codex crates and `tt-domain`.
- `tt-git` depends on git helpers and `tt-domain`.
- `tt-daemon` depends on `tt-store`, `tt-codex`, `tt-git`, and `tt-domain`.
- `tt-ui-core` depends on `tt-domain` and daemon-facing query types, not direct DB access.
- `tt-tui` and future web clients depend on `tt-ui-core`.
- `tt-cli` depends on daemon clients only.

No crate should simultaneously own:

- storage internals
- transport protocol definitions
- operator policy
- UI rendering

## `.tt` Overlay State

`.tt` is a TT-owned overlay keyed primarily by `codex_thread_id`.

The first schema should include:

- `projects`
- `work_units`
- `work_unit_dependencies`
- `thread_bindings`
- `workspace_bindings`
- `review_runs`
- `todo_ledgers`
- `merge_runs`
- `reconcile_checkpoints`

Design rules:

- store TT metadata only
- reference Codex thread ids instead of copying transcript state
- cache only derived indexes needed for TT queries or UI performance

## Migration

TT v2 is now the active implementation line.

The remaining low-level helper crates exist only where v2 still depends on them directly.

### v2 migration rules

- do not build new features on top of legacy compatibility bridges once a v2 equivalent exists
- keep the v2 command surfaces and daemon APIs canonical
- retire any remaining helper dependency only after a v2-native replacement exists

## Initial Delivery Sequence

1. Build `tt-domain`, `tt-store`, `tt-codex`, and `tt-git`.
2. Build `tt-daemon` with a small local IPC surface.
3. Build `tt-ui-core`.
4. Build `tt-tui`.
5. Build `tt-cli`.
6. Continue cutting over operator workflows until only v2 surfaces remain.

## Explicit Non-Goals For Initial v2

- reviving `operator-web`
- preserving the full legacy CLI shape
- remote inbox, delivery, and notification parity
- replacing Codex app-server with TT-owned transport
- duplicating Codex rollout storage under `.tt`
