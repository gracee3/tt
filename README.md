![TT Logo](assets/orcas_banner.png)

# TT

TT is a local supervisor for Codex-driven development. It keeps project state,
thread coordination, and workspace policy close to the checkout so operators
can inspect and steer work without losing the runtime context.

## What It Does

- `tt-daemon` owns the local API boundary and the durable overlay state.
- `tt-cli` is the thin command-line client over the daemon.
- `tt open` uses repo-local Codex state in `.codex/`, resumes the director thread, and hands off into the installed Codex TUI in an interactive terminal. The Codex TUI owns any required login flow.
- `tt-tui` is the internal dashboard / diagnostic surface.
- `.codex/` is the repo-local Codex home used by TT-managed sessions.
- `.tt/` stores repo-local project policy, plan text, runtime state, and local env overrides.
- `tt-git` handles repo/worktree and merge-readiness inspection.

The repo is set up for local development of TT itself. The checked-in
`.tt/project.toml` pins the preferred dev entrypoint to `./target/debug/tt-cli`.

## Reference `.tt/`

This checkout keeps a reference managed-project scaffold in `.tt/`:

- `.tt/project.toml` for repo-local policy and liveness defaults
- `.tt/plan.toml` for the current director plan
- `.tt/state.toml` for runtime bindings, control state, scenario progress, and checksums of the source files
- `.tt/worktrees/` for role worktree checkouts such as `.tt/worktrees/dev/`
- `.tt/settings.env` for repo-local env defaults in this checkout
- `.tt/contract.md` for the worker contract
- `.tt/tt-daemon.sock` for the repo-local TT daemon socket
- `.tt/codex-app-server.log` for the repo-local Codex app-server log

Runtime-only state such as `.tt/overlay.db`, `.tt/tt-daemon.sock`, and
`.tt/codex-app-server.log` stays ignored.

Use `tt clean` to tear down the live managed-project runtime while keeping the checked-in policy scaffold. Use `tt clean --all` to also prune repo-local Codex runtime artifacts such as auth, sessions, sqlite files, and logs while preserving tracked `.codex/config.toml` and `.codex/agents/`.

## Development

```bash
cargo build -p tt-cli -p tt-daemon
./target/debug/tt-cli status
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
