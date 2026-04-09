# Managed Projects

`tt project open --cwd <repo-root>` bootstraps a TT-managed repo for a
director-led project workflow.

This writes the project contract and agent definitions so Codex can spawn the
roles directly. The director is the operator-facing coordinator, and the
workers report only to the director.

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

- `director`: speaks with the operator, turns intent into a plan, and dispatches workers
- `dev`: implements the assigned slice only
- `test`: validates the assigned branch and reports failures exactly
- `integration`: prepares landing and merge readiness

Workers do not coordinate peer-to-peer. All assignments, clarifications, and
escalations flow through the director.

## Lifecycle

- `tt project init --path <target-dir>` creates a fresh git repo plus a minimal
  managed-project scaffold for a supported template.
- `tt project open` writes the manifest, contract, and agent files.
- `tt project inspect` / `tt project status` reads back the manifest, role
  bindings, worktrees, and repository state without changing anything.
- `tt project director` creates or reuses the scaffold, chooses the role
  topology, activates the selected roles in one shot, and can optionally run a
  built-in seeded scenario.
- `tt project spawn` starts live Codex threads for the selected roles and
  records the resulting thread ids in `.tt/managed-project.toml`.
- `tt project attach` binds existing Codex thread ids to the corresponding
  managed-project roles.

The director owns the phase flow:

- `plan` and `todo` turn operator intent into bounded work
- `dispatch` assigns roles and worktrees
- `develop` implements the assigned slice
- `test` validates the change
- `integrate` prepares landing
- `merge` pauses for operator approval before final land or cleanup

Example:

```bash
tt project init --path /tmp/taskflow --template rust-taskflow
tt project open --cwd /path/to/repo
tt project inspect --cwd /path/to/repo
tt project status --cwd /path/to/repo
tt project director --cwd /path/to/repo
tt project director --cwd /path/to/repo --scenario rust-taskflow-four-round --seed-file /path/to/seed.toml
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
- The managed-project manifest records the role/worktree/thread layout plus the
  director-managed attachment state so the project can move from scaffolded to
  partially or fully attached without regenerating the workspace layout

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

## Seeded Scenarios

The built-in seeded scenarios use real Codex threads and a TT-owned round state
record in the managed-project manifest.

- `rust-taskflow-four-round`
  - seeds the director with an initial operator prompt
  - records deterministic structured handoffs for `dev`, `test`, and `integration`
  - stores a deterministic landing approval before the final round completes
- `rust-taskflow-integration-pressure`
  - uses the same taskflow project shape
  - introduces a deterministic integration blocker in round 3
  - verifies the director carries that blocker into the final landing round
