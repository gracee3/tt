# Scenario: Live Concurrent Lanes

## Goal

Prove that two real tracked-thread worktree lanes can run concurrently without crossing lane identity, workspace state, report lineage, or decision lineage.

This scenario uses two tracked-thread lanes in the same run, two separate worktrees, two separate work units, and two bounded live worker turns that overlap in time.

## What Is Seeded

1. A tiny git repository is materialized for each lane under the scenario worktree output root.
2. Each repository contains the same tiny C fixture and shell test, but each lane is instructed to land a different expected greeting string.
3. Orcas daemon state is started and one workstream is created.
4. Two work units are created in that workstream, one per lane.
5. Two tracked-thread records are created, each with its own repository root, worktree path, branch name, base ref, landing target, and cleanup policy before any live assignment begins.
6. The harness inspects the workstream runtime before execution and expects zero active lane threads at that point.

## What Is Live

- Lane A is a real live worker turn on its own tracked-thread/worktree lane.
- Lane B is a real live worker turn on its own tracked-thread/worktree lane.
- Both assignments are launched in the same run before the harness waits for either one, so their active windows overlap.
- Each lane produces a real persisted report, a real final decision, and a real tracked-thread record.
- The workstream runtime surfaces both lanes as managed Codex threads rather than unmanaged runtime threads.

## Live Boundary

- Before execution begins, the harness may create the tiny fixtures, create the workstream and work units, and create the two tracked-thread records with their workspace contracts.
- After the live worker turns begin, the harness must not patch either source tree, seed reports, seed decisions, fake a tracked-thread binding update, or fake the second lane.
- The harness may inspect CLI-visible state and apply the final completion decision for each lane.

## What This Proves

- Orcas can keep two tracked-thread ids distinct in the same run.
- Orcas can keep two worktree paths and branch identities distinct in the same run.
- Orcas can keep report and decision lineage isolated by lane.
- Orcas can let two live assignments overlap without cross-lane contamination.
- Orcas can surface both lanes on the same workstream runtime as two managed threads with no unmanaged runtime thread leak.
- Orcas can complete both lanes cleanly using persisted evidence from each lane independently.

## Pass Conditions

- Lane A and lane B have different tracked-thread ids.
- Lane A and lane B have different worktree paths and branch names.
- The workstream runtime exists before execution with zero active lane threads, then shows exactly two managed lane threads and no unmanaged external thread after both live assignments run.
- Each lane produces a persisted report linked to the correct assignment and work unit.
- Each lane changes only its own expected bounded files and does not pick up the other lane’s string.
- Each lane’s final decision is recorded against the correct work unit and report.
- The same lane identity appears before execution, after the report, and after the final decision.
- No report, decision, or tracked-thread record from one lane is attached to the other lane.

## Fail Conditions

- The two lanes share a tracked-thread id, worktree path, or branch name.
- The workstream runtime does not reflect the two-lane managed-thread model.
- A report or decision from one lane is attached to the other lane.
- One lane writes into the other lane’s worktree.
- Either lane fails to produce a persisted report.
- Either lane fails to produce a final completion decision.
- The final persisted tracked-thread state contradicts the observed lane identity or requires a harness-side rebind.

## Why This Exists In Addition To The Other Live Scenarios

- `live-worker-direct-patch` proves one live worker turn can land a bounded fix.
- `live-supervisor-micro-proposal` proves a real report can drive a proposal and approval.
- `live-reject-redirect` proves operator redirect can create a corrected next assignment.
- `live-restart-resume` proves recovery after daemon interruption.
- `live-worktree-lifecycle` proves the tracked-thread worktree lifecycle and cleanup path.
- `live-multi-phase-lane` proves one tracked-thread/worktree lane can survive multiple live phases.
- This scenario proves two live lanes can run in the same run without crossing identity or lineage.

## Cleanup Expectations

This scenario stops at final completion decisions for both lanes. It does not prune the worktrees. The key result is that both lanes complete independently and remain inspectable.

## Known Flake Points

- Live Codex latency can vary.
- The local model can take a while to generate each worker report.
- The strongest durability checks are the persisted report objects, the lane-specific worktree diffs, and the lane-specific final decision records.
- The scenario relies on both lanes staying within their declared worktrees and not creating backup or temp files.

## Why This Stops At Completion

The purpose of this slice is to prove lane isolation under overlapping live activity, not to prove prune or restart behavior again. Once both lanes have produced durable reports and durable completion decisions, the scenario has validated the isolation boundary.
