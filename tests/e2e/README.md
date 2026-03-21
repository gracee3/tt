# Orcas E2E Harness

This directory holds the checked-in end-to-end harness for Orcas.

The goal is to keep the normal developer path fast while still making the operator workflows reproducible and inspectable when you want them.

## Layout

- `tests/e2e/bin/orcas.sh` wraps the local Orcas CLI with repo-local XDG paths.
- `tests/e2e/lib/common.sh` centralizes path setup, scenario metadata loading, selection, and shared helpers.
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
make test-e2e TAG=deterministic
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
- `repo-local-logs`
- `proposals-decisions`
- `supervisor-planning`
- `git-worktrees`
- `phased-fibonacci`
- `live-worker-direct-patch`
- `live-supervisor-micro-proposal`

The current migration keeps the existing behavior first, then opens the path for a truly live worker-turn scenario next.
