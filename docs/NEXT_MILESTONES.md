# Next Milestones

- The daemon socket transport is live. The request surface now includes Codex thread lifecycle wiring, workspace actions, daemon-state-first lifecycle operations, and managed-project bootstrap; the next step is richer drill-down/selection ergonomics and cleanup of the remaining compatibility surfaces.
- The TUI now talks through the daemon helper and exposes the new workspace action/lifecycle commands; the next target is more ergonomic drill-down and fewer raw internal aliases in the interactive shell.
- The raw store-shaped CRUD/status commands are isolated behind the CLI `records` namespace; continue trimming the compatibility surfaces toward lifecycle and reconcile flows.
- `tt-git` owns repo/worktree discovery and merge-readiness inspection; keep using it as the source of truth for landing policy and cleanup decisions.
- The last helper layer is gone; keep new transport or protocol work inside v2 crates only and do not add back a separate helper layer.
