# Orcas Realign Todo

## Landed In `orcas-realign-v1`

- Added singular `orcas workstream ...` and role-first `orcas codex spawn ...` surfaces.
- Added global typed Codex config sections for shared app-server, responses, direct API, profiles, and model providers.
- Moved shared app-server lifecycle into `orcas app-server ...` commands.
- Cut `orcasd` over to connect-only upstream behavior even when legacy spawn modes are still configured.
- Added checked-in default role files under `roles/<role>/role.md`.

## Follow-up

- Tighten the app-server config persistence model beyond the reserved `default` instance.
- Replace remaining legacy workstream runtime surfaces with the new shared-runtime status model.
- Expand role file schema beyond plain markdown body loading.
- Add richer workstream metadata beyond the execution scope-derived worktree and lane homes.
- Verify whether Codex can safely reuse host auth state from `~/.codex/auth.json` before depending on it.
