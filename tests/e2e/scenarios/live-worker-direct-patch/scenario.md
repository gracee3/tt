# Scenario: Live Worker Direct Patch

## Goal

Verify that Orcas can carry one real live worker turn all the way through to a bounded on-disk code fix and a real report, without the harness writing the fix or seeding the report after execution begins.

## Seeded Before Execution

1. A tiny git repository is materialized under the scenario worktree output root.
2. The repository contains one C source file with a small, obvious bug.
3. A shell-based test script and Makefile are present so the repository can prove the fix locally.
4. Orcas daemon state is started and a single workstream/work unit is created.
5. A direct `assignment/start` request is issued against the fixture repo.

## Live Boundary

- The harness may create the fixture repository, workstream, work unit, and assignment before execution starts.
- After `assignment/start` begins, Orcas must do the actual work.
- The harness must not patch the source file, seed a report, or simulate success after execution begins.

## What This Proves

- Orcas can execute one bounded worker turn in the live lane.
- Orcas can write a real fix to disk.
- Orcas can emit a real report tied to the assignment and work unit.
- The resulting repository change stays bounded to the expected file.

## Pass Conditions

- `assignment/start` returns successfully.
- The assignment reaches its post-turn terminal state for this flow.
- A report exists and is linked to the assignment and work unit.
- The expected source file changes on disk.
- `make test` passes in the fixture repository.
- `git status` shows only the expected bounded change.

## Fail Conditions

- The assignment never starts or times out.
- No report is generated.
- The wrong file changes.
- More than one source file changes.
- The shell test still fails after the turn.

## Known Flake Points

- Live Codex latency can be variable.
- The daemon bootstrap may take a few seconds before the assignment starts.
- If network or Codex availability is interrupted, the scenario fails in the live lane by design.
