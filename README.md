![TT Logo](assets/orcas_banner.png)

# TT

TT is a local supervisor for Codex-driven development. It keeps project state,
thread coordination, and workspace policy close to the checkout so operators
can inspect and steer work without losing the runtime context.

## What It Does

- `tt-daemon` owns the local API boundary and the durable overlay state.
- `tt-cli` is the thin command-line client over the daemon.
- `tt-tui` is the interactive operator surface.
- `.tt/` stores repo-local project policy, plans, and managed-project state.
- `tt-git` handles repo/worktree and merge-readiness inspection.

The repo is set up for local development of TT itself. The checked-in
`.tt/project.toml` pins the preferred dev entrypoint to `./target/debug/tt-cli`.

## Reference `.tt/`

This checkout keeps a reference managed-project scaffold in `.tt/`:

- `.tt/project.toml` for repo-local policy overrides
- `.tt/plan.toml` for the current director plan
- `.tt/state.toml` for managed-project runtime state
- `.tt/settings.env` for repo-local env defaults in this checkout
- `.tt/contracts/worker-contract.md` for the worker contract

Runtime-only state such as `.tt/overlay.db` stays ignored.

## Development

```bash
cargo build -p tt-cli -p tt-daemon
./target/debug/tt-cli status
./target/debug/tt-cli codex app-servers
```

Useful checks:

- `cargo fmt`
- `cargo check`
- `cargo test`

Live and scenario workflows live under `tests/e2e/`.

## Docs

- [TT v2 Architecture](docs/tt_v2_architecture.md)
- [Managed Projects](docs/managed-projects.md)
- [TT / Codex Runtime Contract](docs/tt_codex_runtime_contract.md)
- [Phase 2 Plan](docs/PHASE2_EXECUTION_PLAN.md)

## License

Apache 2.0. See [LICENSE](LICENSE).
