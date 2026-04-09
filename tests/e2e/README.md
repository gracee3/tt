# TT E2E Harness

This directory holds the checked-in end-to-end harness for TT.

The harness is split into deterministic smoke tests and live managed-project scenarios.

## Layout

- `tests/e2e/bin/tt.sh` wraps the local TT CLI with a repo-local `TT_HOME`.
- `tests/e2e/lib/common.sh` centralizes path setup, scenario metadata loading, shared helpers, and live daemon wiring.
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
- `target/e2e/tt/<run-id>/<scenario>/`
- `target/e2e/xdg/<run-id>/<scenario>/`

Managed-project live scenarios also write a scenario-local progress stream at:

- `target/e2e/artifacts/<run-id>/<scenario>/.tt/scenarios/<scenario-id>/progress.jsonl`

## Scenario Metadata

Each scenario provides `scenario.env` with shell-friendly key/value pairs:

- `NAME`
- `MODE` (`deterministic`, `hybrid-live`, `full-live`, or `recovery`)
- `TAGS` as a comma-separated list
- `DEFAULT_ENABLED` as `true` or `false`
- `TIMEOUT_SECONDS`
- `REQUIRES_RUNTIME`
- `REQUIRES_NETWORK`
- `REQUIRES_CLEAN_GIT`
- `REQUIRES_EXTRACTED_HANDOFFS`
- `EXPECTED_LONG_BUILD`
- `REQUIRES_PROGRESS_UPDATES`
- `SOFT_SILENCE_SECONDS`
- `HARD_CEILING_SECONDS`

Metadata is validated before execution. Missing or malformed metadata fails the run.

## Running Scenarios

Run the default deterministic lane:

```bash
make test-e2e
```

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
make test-e2e TAG=managed-project
```

You can also invoke the runner directly:

```bash
tests/e2e/run_all.sh
tests/e2e/run_scenario.sh tests/e2e/scenarios/hello
```

## Cleanup

Remove all generated E2E output with:

```bash
make clean-e2e
```

That removes `target/e2e/` and nothing else.

## Current Scenarios

- `hello`
- `managed-project-git-worktree`
- `managed-project-rust-taskflow-four-round`
- `managed-project-rust-taskflow-integration-pressure`

The managed-project scenarios exercise the current TT v2 workflow:

- create a new managed project repo from TT
- activate director/dev/test/integration roles
- run a seeded multi-round Rust project scenario
- run a seeded multi-round Rust project scenario with integration pressure
- inspect attachment state through the daemon
- inspect the JSONL progress stream while the director/subagents are running
- verify the commands work from a linked git worktree path
