# Orcas E2E Harness

This directory holds the checked-in end-to-end harness for Orcas.

The goal is to keep the normal developer path fast while still making the operator workflows reproducible and inspectable when you want them.

## Layout

- `tests/e2e/bin/orcas.sh` wraps the local Orcas CLI with a repo-local `ORCAS_HOME`.
- `tests/e2e/lib/common.sh` centralizes path setup, scenario metadata loading, selection, shared helpers, and legacy XDG shim paths for older scenarios.
- `tests/e2e/run_all.sh` discovers and runs scenarios by metadata.
- `tests/e2e/run_scenario.sh` runs one scenario by name or path.
- `tests/e2e/scenarios/<name>/scenario.md` explains the scenario.
- `tests/e2e/scenarios/<name>/scenario.env` declares selection metadata.
- `tests/e2e/scenarios/<name>/run.sh` executes the scenario.

Generated output is written only under `target/e2e/`:

- `target/e2e/logs/<run-id>/<scenario>/`
- `target/e2e/reports/<run-id>/<scenario>/`
- `target/e2e/artifacts/<run-id>/<scenario>/`
- `target/e2e/worktrees/<run-id>/<scenario>/`
- `target/e2e/orcas/<run-id>/<scenario>/`
- `target/e2e/xdg/<run-id>/<scenario>/`

## Scenario Metadata

Each scenario must provide `scenario.env` with shell-friendly key/value pairs:

- `NAME`
- `MODE` (`deterministic`, `hybrid-live`, `full-live`, or `recovery`)
- `TAGS` as a comma-separated list
- `DEFAULT_ENABLED` as `true` or `false`
- `TIMEOUT_SECONDS`
- `REQUIRES_CODEX`
- `REQUIRES_NETWORK`
- `REQUIRES_CLEAN_GIT`

Metadata is validated before execution. Missing or malformed metadata fails the run.

Lane contract:

- the default daily deterministic lane is expected to work from a normal dirty developer checkout
- scenarios that require a clean git tree are opt-in and must not be default-enabled

Aligned live lane contract for the current tracked-thread/workspace migration:

- lane-centric live scenarios create the workstream first, then declare the tracked-thread workspace before the first live assignment
- the harness inspects the workstream runtime through `orcas workstreams runtime get` and `orcas codex threads list --workstream`
- the first live assignment must auto-bind into the declared tracked-thread lane; the harness must not repair binding manually after the report appears
- the currently aligned scenarios are `live-worker-direct-patch`, `live-worktree-lifecycle`, `live-multi-phase-lane`, and `live-concurrent-lanes`

## Running Scenarios

Run the default deterministic lane:

```bash
make test-e2e
```

That default lane is the daily confidence path. It should stay usable from an ordinary in-progress checkout.

Run the live-only lane:

```bash
make test-e2e-live
```

Run the long lane:

```bash
make test-e2e-long
```

Run one scenario:

```bash
make test-e2e SCENARIO=hello
```

Filter by tag:

```bash
make test-e2e TAG=deterministic
```

You can also invoke the runner directly:

```bash
tests/e2e/run_all.sh
tests/e2e/run_scenario.sh tests/e2e/scenarios/hello
```

### Shared UI Lab For Live Scenarios

If you want to launch live scenarios from the terminal and inspect them immediately in the operator web UI, use the shared UI lab instead of the default scenario-local XDG roots.

The lab uses a dedicated Orcas state under `target/ui-e2e-lab/`, so it does not overwrite your normal `~/.orcas` state.

Reset and start the lab:

```bash
tests/e2e/bin/ui_lab_live.sh restart
```

Run one supported live scenario into that shared lab:

```bash
tests/e2e/bin/ui_lab_live.sh run live-worker-direct-patch
```

Run the supported shared-lab suite sequentially:

```bash
tests/e2e/bin/ui_lab_live.sh run-all
```

Supported shared-lab scenarios:

- `live-worker-direct-patch`
- `live-supervisor-micro-proposal`
- `live-reject-redirect`
- `live-worktree-lifecycle`
- `supervisor-planning`

These scenarios reuse the existing lab daemon and lab XDG roots, so their workstreams, work units, assignments, reports, and bound threads appear in the UI immediately.

Not every live scenario is suitable for the shared lab. Scenarios that intentionally restart or stop the daemon, such as `live-restart-resume`, should still be run in their normal isolated mode.

### Local Supervisor Models

Only scenarios that actually generate a live supervisor proposal require a local OpenAI-compatible supervisor endpoint.

Current live supervisor proposal scenarios:

- `supervisor-planning`
- `live-supervisor-micro-proposal`
- `live-reject-redirect`
- `live-multi-phase-lane`

These scenarios are opt-in. They are not part of the default daily deterministic lane.

Seeded proposal scenarios remain model-free:

- `proposals-decisions`
- `phased-fibonacci`

These seeded scenarios now round-trip their `state.json` inputs through the same `StoredState`
serializer/deserializer path the daemon uses, so deterministic fixtures stay aligned with the
live persisted collaboration shape.

For live supervisor proposal scenarios, export:

```bash
export ORCAS_E2E_SUPERVISOR_BASE_URL="http://127.0.0.1:8000/v1"
export ORCAS_E2E_SUPERVISOR_MODEL="gpt-oss-20b"
export ORCAS_E2E_SUPERVISOR_API_KEY_ENV=""
export ORCAS_E2E_SUPERVISOR_REASONING_EFFORT=""
export ORCAS_E2E_SUPERVISOR_MAX_OUTPUT_TOKENS="2048"
```

The scenario runner probes `.../models` before it starts. If those variables are unset, or if the endpoint is unreachable, the scenario fails immediately with an actionable message instead of silently depending on a hidden local model setup.

The harness still accepts the older `ORCAS_E2E_QWEN_*` variable names as a fallback, but the intended contract is provider-neutral: any local OpenAI-compatible endpoint is acceptable for these opt-in proposal scenarios.

## Cleanup

Remove all generated E2E output with:

```bash
make clean-e2e
```

That removes `target/e2e/` and nothing else.

## Current Scenarios

- `hello`
- `repo-local-logs`
- `proposals-decisions`
- `supervisor-planning`
- `git-worktrees`
- `phased-fibonacci`
- `live-worker-direct-patch`
- `live-supervisor-micro-proposal`
- `live-reject-redirect`
- `live-restart-resume`
- `live-worktree-lifecycle`
- `live-multi-phase-lane`
- `live-concurrent-lanes`

The current migration now treats the workstream runtime as the app-server ownership unit and the tracked-thread workspace as the execution lane. The aligned live scenarios prove first-assignment lane binding, tracked-thread/worktree continuity, tracked-thread lifecycle transitions, and concurrent lane isolation on top of that model. Other live proposal/redirect/restart scenarios still follow as a separate migration slice.
