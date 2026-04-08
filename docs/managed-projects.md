# Managed Projects

`tt project open --cwd <repo-root>` bootstraps a TT-managed repo for the
director/dev/test/integration workflow.

This writes the project contract and agent definitions so Codex can spawn the
roles directly. It does not itself create live Codex threads.

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

## Default Topology

- The current checkout stays on the base branch
- `dev`, `test`, and `integration` get sibling worktrees under
  `.tt-worktrees/<project-slug>/`
- Branch names default to `tt/<project-slug>/<role>`
- The managed-project manifest records the role/worktree layout so the
  director can later attach real Codex threads to the right workspaces

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
