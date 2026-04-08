# TT v2 Phase 2: Daemon IPC, Interactive TUI, Thin CLI

## Summary

Build the next v2 slice on top of the current checkpoint: turn `tt-daemon` into the local API boundary, make `tt-tui` interactive, and reduce `tt-cli` to a narrow client over the same daemon surface.

This phase keeps the established architecture:

- Codex remains the runtime/source of truth for threads, turns, rollouts, and app-server transport.
- TT remains the overlay/orchestration layer for `.tt`, repo/worktree policy, workspace bindings, and operator workflows.
- `tt-git` remains the authority for repo/worktree inspection and merge readiness.

## Key Changes

- Add a small daemon request/response surface with query and mutation operations for:
  - project CRUD
  - work unit CRUD
  - thread binding management
  - workspace binding management
  - merge run queries and repository summaries
- Keep `tt-daemon` thin:
  - no UI rendering
  - no duplicate transcript/session storage
  - no new execution runtime
- Make `tt-tui` interactive:
  - select and inspect projects, work units, threads, and workspaces
  - show repo/worktree status from `tt-git`
  - trigger basic workspace and inspection actions from the dashboard
- Rework `tt-cli` into a thin daemon client:
  - commands map directly to daemon operations
  - no business logic duplication in the CLI
- Extend Codex integration only through adapters:
  - thread list/read/start/resume
  - `.codex` discovery
  - catalog/index lookups for operator views

## Test Plan

- Daemon tests:
  - query/update flows operate on the `.tt` store
  - repository summaries are returned for git checkouts and omitted outside repos
  - workspace binding state stays consistent across updates
- TUI tests:
  - dashboard rendering still works
  - interactive actions call daemon APIs instead of touching storage directly
- CLI tests:
  - commands map to daemon requests and produce expected summaries
- Integration checks:
  - `cargo test -p tt-daemon -p tt-tui -p tt-cli`
  - `cargo check -p tt-daemon -p tt-tui -p tt-cli`
  - `cargo run -p tt-tui --quiet`
  - `cargo run -p tt-cli --quiet -- status`

## Assumptions

- `tt-git` remains the repo/worktree authority and the daemon delegates to it rather than duplicating git logic.
- The first IPC layer can be local-only and minimal; remote/operator-web parity stays out of scope.
- Existing v2 checkpoint commits remain intact; this phase builds on them without rewinding the rewrite.
