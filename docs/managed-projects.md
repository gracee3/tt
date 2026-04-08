# Managed Projects

`tt project open --cwd <repo-root>` bootstraps a TT-managed repo for the
director/dev/test/integration workflow.

This writes the project contract and agent definitions so Codex can spawn the
roles directly. Live threads are started separately with `tt project spawn` or
bound later with `tt project attach`.

## What It Creates

- `.codex/config.toml` with the project agent limits
- `.codex/agents/director.toml`
- `.codex/agents/dev.toml`
- `.codex/agents/test.toml`
- `.codex/agents/integration.toml`
- `.tt/contracts/worker-contract.md`
- `.tt/managed-project.toml`
- one worktree each for `dev`, `test`, and `integration`

## Role Model

- `director`: coordinates the project, assigns work, and manages handoffs
- `dev`: implements the assigned slice only
- `test`: validates the assigned branch and reports failures exactly
- `integration`: prepares landing and merge readiness

## Lifecycle

- `tt project open` writes the manifest, contract, and agent files.
- `tt project spawn` starts live Codex threads for the selected roles and
  records the resulting thread ids in `.tt/managed-project.toml`.
- `tt project attach` binds existing Codex thread ids to the corresponding
  managed-project roles.

Example:

```bash
tt project open --cwd /path/to/repo
tt project spawn --cwd /path/to/repo
tt project spawn --cwd /path/to/repo --role dev --role test
tt project attach --cwd /path/to/repo \
  --binding director=<thread-id> \
  --binding dev=<thread-id> \
  --binding test=<thread-id> \
  --binding integration=<thread-id>
```

## Default Topology

- The current checkout stays on the base branch
- `dev`, `test`, and `integration` get sibling worktrees under
  `.tt-worktrees/<project-slug>/`
- Branch names default to `tt/<project-slug>/<role>`
- The managed-project manifest records the role/worktree/thread layout so the
  director can move a project from scaffolded to partially or fully attached
  without regenerating the workspace layout

## Contract

Worker communication is defined by `.tt/contracts/worker-contract.md`.
Each handoff should include:

- `status`
- `changed_files`
- `tests_run`
- `blockers`
- `next_step`

The contract is intentionally explicit so the director can coordinate workers
with prompts, skills, and agent definitions without relying on hidden state.
