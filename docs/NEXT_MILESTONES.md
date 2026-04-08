# Next Milestones

- Extend the TT daemon beyond status/count queries into thread/workspace orchestration, local IPC, and merge-aware repo summaries.
- Add a minimal TUI on top of the new daemon dashboard summary. A first runnable `tt-tui` dashboard entrypoint now exists; next step is interactivity and workspace actions.
- `tt-git` now owns repo/worktree discovery and merge-readiness inspection; use it as the source of truth for landing policy and cleanup decisions.
- Keep the TT contract snapshot in `crates/tt-runtime/contracts/tt-contract-index.json` aligned with the local TT checkout.
- Add any future TT-facing typed fields to `tt-runtime::contract` before wiring them into runtime code.
- When TT updates land upstream, regenerate the snapshot with `cargo run -p tt-runtime --bin tt-contract-sync -- --out crates/tt-runtime/contracts/tt-contract-index.json` and run the contract drift test.
