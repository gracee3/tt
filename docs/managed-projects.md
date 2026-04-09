# Managed Projects

`tt project open --cwd <repo-root>` bootstraps a TT-managed repo for a
director-led project workflow.

This writes the project contract and agent definitions so Codex can spawn the
roles directly. The director is the operator-facing coordinator, and the
workers report only to the director.
The live workflow uses explicit subagents, not anonymous background threads:
the director opens first, prints the environment and plan, and then spawns the
worker subagents for the project.
The runtime also writes a scenario-level JSONL progress stream so the operator
can see what the director and subagents are doing while a run is live.

## What It Creates

- `.codex/config.toml` with the project agent limits
- `.codex/agents/director.toml`
- `.codex/agents/dev.toml`
- `.codex/agents/test.toml`
- `.codex/agents/integration.toml`
- `.tt/project.toml` with repo-local project policy and liveness overrides
  - optional `tt_runtime_bin` can pin the preferred repo-local `tt-cli` path for development checkouts such as `./target/debug/tt-cli`
- `.tt/plan.toml` with the director's current plan and checkpoints
- `.tt/contracts/worker-contract.md`
- `.tt/state.toml`
- `.tt/settings.env` with repo-local env defaults such as `TT_CODEX_BIN` and `TT_CODEX_APP_SERVER_BIN`
- `.tt/scenarios/<scenario-id>/progress.jsonl` for live director/subagent progress
- one worktree each for `dev`, `test`, and `integration`

## Role Model

- `director`: speaks with the operator, turns intent into a plan, and dispatches workers
- `dev`: implements the assigned slice only
- `test`: validates the assigned branch and reports failures exactly; it uses `danger-full-access` so build tools like `cargo test` can create their target artifacts without sandbox namespace issues
- `integration`: prepares landing and merge readiness

Default managed-project model roster:

- `director`: `gpt-5.4` with `model_reasoning_effort = "medium"`
- `dev`: `gpt-5.4-mini` with `model_reasoning_effort = "medium"`
- `test`: `gpt-5.4-mini` with `model_reasoning_effort = "medium"`
- `integration`: `gpt-5.4-mini` with `model_reasoning_effort = "medium"`

Workers do not coordinate peer-to-peer. All assignments, clarifications, and
escalations flow through the director.
The managed-project runtime also writes a JSONL progress stream so the operator
can inspect what the director and subagents are doing while a run is live.

## Lifecycle

- `tt project init --path <target-dir>` creates a fresh git repo plus a minimal
  managed-project scaffold for a supported template.
- `tt project open` writes the manifest, contract, and agent files.
- `tt project open` writes the repo-local project policy and plan sidecars.
- `tt project open` respects an existing repo-local `settings.env` overlay when present.
- `tt project inspect` / `tt project status` reads back the manifest, role
  bindings, worktrees, repository state, and effective project plan without
  changing anything.
- `tt project control --cwd /path/to/repo --role <role> --mode <director|manual_next_turn|manual|director_paused>`
  updates the per-thread control mode for a managed role without changing the
  thread binding itself.
- `tt project plan show` prints the current plan artifact.
- `tt project plan refresh` reloads the current managed-project plan and
  renders the effective state without mutating the on-disk artifact.
- `tt project director` creates or reuses the scaffold, chooses the role
  topology, activates the selected roles in one shot, and can optionally run a
  built-in seeded scenario. The director plan-checks against `.tt/plan.toml`
  before it dispatches workers and uses the first planning round to resolve
  scope, validation, merge policy, and repo-specific constraints before
  dispatch.
- `tt project spawn` starts live Codex threads for the selected roles and
  records the resulting thread ids in `.tt/state.toml`.
- `tt project attach` binds existing Codex thread ids to the corresponding
  managed-project roles.

The director owns the phase flow:

- `plan` and `todo` turn operator intent into bounded work
- `dispatch` assigns roles and worktrees
- `develop` implements the assigned slice
- `test` validates the change
- `integrate` prepares landing
- `merge` pauses for operator approval before final land or cleanup

The planning round is intentionally explicit. The director records the repo
local plan questions, risks, and constraints in `.tt/plan.toml` before workers
are dispatched so later rounds can reuse the same project-specific decisions.

Example:

```bash
tt project init --path /tmp/taskflow --template rust-taskflow
tt project open --cwd /path/to/repo
tt project control --cwd /path/to/repo --role dev --mode manual_next_turn
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
- The managed-project state file records the role/worktree/thread layout plus
  the director-managed attachment state so the project can move from scaffolded
  to partially or fully attached without regenerating the workspace layout
- `--director_model`, `--dev_model`, `--test_model`, and `--integration_model`
  override the default model string for that role while keeping reasoning
  effort at `medium`

Each managed thread has a control mode:

- `director`: TT/director owns the next turn
- `manual_next_turn`: the director pauses before the next turn so the operator
  can take over in Codex TUI
- `manual`: the operator controls the thread until TT switches it back
- `director_paused`: the director is intentionally paused for this thread

`tt project inspect` shows the current control mode for each role, and the
director resumes automatic dispatch once a role returns to `director`.

Managed-project worker roles default to `danger-full-access` so live builds and
integration tests do not trip over sandbox setup in the Codex app-server.

## Contract

Worker communication is defined by `.tt/contracts/worker-contract.md`.
Each handoff should include:

- `status`
- `changed_files`
- `tests_run`
- `blockers`
- `next_step`

The contract is intentionally explicit so the director can coordinate workers
with prompts, skills, agent definitions, and repo-local project policy without
relying on hidden state.

## Seeded Scenarios

The built-in seeded scenarios use real Codex threads and a TT-owned round state
record in the managed-project state file.

- `rust-taskflow-four-round`
  - seeds the director with an initial operator prompt
  - records deterministic structured handoffs for `dev`, `test`, and `integration`
  - stores a deterministic landing approval before the final round completes
- `rust-taskflow-integration-pressure`
  - uses the same taskflow project shape
  - introduces a deterministic integration blocker in round 3
  - verifies the director carries that blocker into the final landing round
