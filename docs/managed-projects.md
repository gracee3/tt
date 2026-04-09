# Managed Projects

TT now exposes a minimal public operator flow:

- `tt init`
- `tt open`
- `tt status`
- `tt codex`
- `tt docs export-cli`

`tt init` bootstraps the current repo as a managed project. `tt open` resumes the
director-led workflow for that repo.

## Public Workflow

Initialize a repo in the current directory:

```bash
tt init --title "Taskflow" --objective "Build a Rust workflow runner"
```

Initialize a fresh path:

```bash
tt init --path /tmp/taskflow --template rust-taskflow
```

Resume the director-managed workflow:

```bash
tt open
```

Generate the public CLI reference:

```bash
tt docs export-cli --output docs/cli-reference.md
```

## What `tt init` Creates

- `.codex/config.toml`
- `.codex/agents/director.toml`
- `.codex/agents/dev.toml`
- `.codex/agents/test.toml`
- `.codex/agents/integration.toml`
- `.tt/project.toml`
- `.tt/plan.toml`
- `.tt/contracts/worker-contract.md`
- `.tt/state.toml`
- `.tt/settings.env`

Managed-project live runs also write scenario artifacts under `.tt/scenarios/`.

## Role Model

- `director`: speaks with the operator, plans the work, dispatches subagents, and owns approvals
- `dev`: implements the assigned slice
- `test`: validates the assigned branch and reports exact failures
- `integration`: prepares landing and merge readiness

Default managed-project model roster:

- `director`: `gpt-5.4` with `model_reasoning_effort = "medium"`
- `dev`: `gpt-5.4-mini` with `model_reasoning_effort = "medium"`
- `test`: `gpt-5.4-mini` with `model_reasoning_effort = "medium"`
- `integration`: `gpt-5.4-mini` with `model_reasoning_effort = "medium"`

Workers report only to the director.

## Internal Surface

Legacy low-level commands were removed from the public CLI. TT still keeps a
hidden internal surface for e2e and debugging:

- `tt internal project ...`
- `tt internal workspace ...`
- `tt internal records ...`
- `tt internal repo`

Those commands are intentionally excluded from normal help and from the
generated public CLI reference.
