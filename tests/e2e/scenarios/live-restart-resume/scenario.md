# Scenario: Live Restart Resume

## Goal

Verify that Orcas can survive a daemon interruption during a live worker turn and still reconcile back to the correct persisted state after restart.

## What Is Seeded

1. A tiny git repository is materialized under the scenario worktree output root.
2. The repository contains one C source file with the same small greeting bug used by the other live scenarios.
3. A shell-based test script and Makefile are present so the repository can prove the fix locally.
4. The scenario writes a repo-local Orcas config that can target a local vLLM Responses endpoint by default, while still allowing an explicit hosted API override.
5. Orcas daemon state is started and a single workstream/work unit is created.
6. A tracked-thread record is created before the first live assignment, with a declared repository root, worktree path, branch name, base ref, landing target, and cleanup policy.
7. The harness inspects the workstream runtime before execution and expects zero live lane threads at that point.

## What Is Live

- The first worker turn is real and live.
- The daemon interruption is real and happens while the live workflow is in flight.
- The daemon restart is real and uses the same persisted local state/XDG paths.
- The resumed workflow is reconciled by Orcas from persisted/upstream truth, with `turns get` acting as the recovery probe after restart.
- The final persisted assignment/report state is produced by Orcas, not seeded by the harness.

## What Is Intentionally Interrupted

- The daemon is stopped after the live worker turn is visibly active and before the run has converged to its final persisted state.
- The harness does not stop after a successful turn completion; it interrupts mid-workflow to test recovery.

## Live Boundary

- Before execution begins, the harness may create the fixture repository, workstream, work unit, tracked-thread workspace contract, and initial assignment.
- After `assignment/start` begins, the harness may only observe state, stop the daemon, restart the daemon, and verify durable state.
- The harness must not patch the source file, seed a report, or rewrite persisted state to simulate recovery.

## What This Proves

- Orcas can survive a controlled daemon interruption during a live assignment.
- Orcas can bind the first live assignment directly into the predeclared tracked-thread workspace lane and recover that same lane after restart.
- Orcas can restart against the same persisted local state.
- Orcas can reconcile the in-flight workflow from durable/upstream truth instead of duplicating or losing it.
- Orcas can converge to the correct persisted result after restart.

## Pass Conditions

- A real live assignment is created.
- There is clear evidence the live turn started before interruption.
- The tracked-thread record exists before execution, auto-binds to the live upstream thread, and remains bound to that same lane after restart.
- The workstream runtime shows exactly one managed lane thread and no unmanaged external thread before and after restart.
- The daemon stops and restarts cleanly against the same local state.
- The original assignment does not split into duplicates or corrupt the work unit state.
- The workflow eventually converges to the correct persisted state.
- If a report exists, it is linked correctly and remains singular.
- The work unit is not left stuck in a transient state forever.
- No incorrect downgrade to `Lost` occurs once terminal evidence exists.
- The fixture still passes `make test`.

## Fail Conditions

- The assignment never actually starts before interruption.
- The tracked-thread lane does not bind automatically or the runtime view is inconsistent with the declared lane before or after restart.
- Daemon restart fails.
- The workflow remains permanently stuck after restart.
- Duplicate reports or assignments are created by restart.
- Terminal evidence exists but the persisted state downgrades incorrectly.
- The final persisted state is inconsistent with the observed live outcome.
- The local fixture test still fails after the live turn.

## Why This Exists In Addition To The Other Live Scenarios

- `live-worker-direct-patch` proves a single live worker turn can land a bounded fix.
- `live-supervisor-micro-proposal` proves the proposal/approval branch.
- `live-reject-redirect` proves governed operator redirect.
- This scenario proves interruption and recovery across those same live semantics.

## Known Flake Points

- Live worker latency can vary.
- Recovery depends on the daemon being able to reconnect to the surviving upstream session or otherwise reconcile persisted state cleanly.
- The daemon stop/start window is intentionally short, so timing can vary slightly across machines.
- The scenario uses a repo-local config override so it can run against either a local vLLM endpoint or an explicitly configured hosted API.

## Why This Stops At Reconciliation

The purpose of this slice is to prove the smallest restart/recovery boundary:

1. real live worker turn
2. controlled daemon interruption
3. daemon restart
4. resumed reconciliation to correct persisted state

Executing a later supervisor step would prove a different boundary and add avoidable scope.
