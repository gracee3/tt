# Next Milestones

- The daemon socket transport is live. The request surface now includes Codex thread lifecycle wiring, workspace actions, and daemon-state-first lifecycle operations; the next step is richer drill-down/selection ergonomics and cleanup of the remaining legacy surfaces.
- The TUI now talks through the daemon helper and exposes the new workspace action/lifecycle commands; the next target is more ergonomic drill-down and fewer raw internal aliases in the interactive shell.
- The raw store-shaped CRUD/status commands are isolated behind the CLI `legacy` namespace; continue trimming the non-legacy surfaces toward lifecycle and reconcile flows.
- `tt-git` owns repo/worktree discovery and merge-readiness inspection; keep using it as the source of truth for landing policy and cleanup decisions.
- Keep the TT contract snapshot in `crates/tt-runtime/contracts/tt-contract-index.json` aligned with the local TT checkout.
- Add any future TT-facing typed fields to `tt-runtime::contract` before wiring them into runtime code.
- When TT updates land upstream, regenerate the snapshot with `cargo run -p tt-runtime --bin tt-contract-sync -- --out crates/tt-runtime/contracts/tt-contract-index.json` and run the contract drift test.
