# Scenario: Live Worktree Lifecycle

## Goal

Verify that Orcas can use a real tracked-thread worktree lane, land a bounded code change there, and carry the workspace through prepare, merge-prep, landing, and cleanup transitions with persisted state that matches the filesystem.

## What Is Seeded

1. A tiny git worktree is materialized under the scenario worktree output root from the Orcas repository itself.
2. The worktree contains a tiny C program, a shell test, and a Makefile with one obvious bug.
3. Orcas daemon state is started and one work unit is created in the workstream.
4. The tracked-thread record points at the declared worktree path, branch name, base ref, landing target, strategy, sync policy, and cleanup policy before any tracked-thread lifecycle step begins.
5. The harness inspects the workstream runtime before the first live assignment and expects the lane to exist only as declared state, not as an active runtime thread yet.

## What Is Live

- The bootstrap worker turn is real and makes the bounded code change in the declared tracked-thread worktree lane.
- The tracked-thread lifecycle steps are real daemon-driven transitions:
  - prepare-workspace
  - merge-prep
  - authorize-merge
  - execute-landing
  - prune-workspace
- The decision calls between those steps are real and reopen the same work unit through persisted operator-facing state, not by seeding the next assignment.
- The final persisted tracked-thread state, landing records, prune records, and decision records are produced by Orcas.

## Live Boundary

- Before execution begins, the harness may create the worktree, populate the tiny fixture files, create the work unit, and create the tracked-thread record with its workspace contract.
- After the first live worker turn begins, the harness must not patch the source file, seed a report, fake any tracked-thread binding update, or fake any landing/cleanup result.
- The harness may inspect CLI-visible state, request the next lifecycle step through the supported decision path, authorize landing, execute landing, and prune the workspace.

## What This Proves

- Orcas can operate inside a tracked-thread worktree lane that is explicitly declared in authority state.
- Orcas can bind that lane automatically on the first live assignment and reflect it immediately through the workstream runtime view.
- Orcas can land a bounded code change into that lane and keep the local git state inspectable.
- Orcas can advance the lane through prepare-workspace to merge-prep to landing authorization to landing execution to prune cleanup.
- Orcas can reopen the lane between steps through persisted decision state without losing the tracked-thread binding.
- Orcas can prune the workspace and leave persisted state that honestly reflects the cleanup outcome.

## Pass Conditions

- The tracked-thread record shows the expected workspace contract before execution.
- The workstream runtime exists before execution with zero active lane threads.
- The bootstrap live turn completes and produces a persisted report.
- The bounded code change lands in the declared worktree and `make test` passes there.
- The bootstrap turn leaves exactly one expected source edit in the worktree and no unexpected churn.
- The tracked-thread binding is present automatically before lifecycle operations begin.
- The workstream runtime shows exactly one managed lane thread and no unmanaged external thread before lifecycle operations begin.
- Prepare workspace completes and produces a persisted report.
- A `Continue` decision reopens the work unit for the next step.
- Merge prep reaches a ready state for landing.
- Landing authorization succeeds and is recorded.
- Another `Continue` decision reopens the work unit for landing execution.
- Landing execution succeeds and is recorded.
- Another `Continue` decision reopens the work unit for prune cleanup.
- Prune workspace succeeds and is recorded.
- The declared worktree path is removed or otherwise reflects the configured cleanup policy.
- A final `MarkComplete` decision closes the lifecycle work unit after prune.
- Final tracked-thread state matches the observed landing/cleanup behavior.

## Fail Conditions

- The tracked-thread record does not show the expected workspace contract.
- The runtime does not reflect the workstream/lane before lifecycle operations begin.
- The bootstrap live turn never reaches a terminal persisted state.
- The code change lands outside the declared worktree path or changes too many files.
- `make test` still fails after the turn.
- The tracked-thread binding is missing or requires a harness-side repair before lifecycle operations begin.
- Prepare workspace, merge prep, landing authorization, landing execution, or prune workspace fails.
- The decision steps do not reopen the work unit.
- The final persisted tracked-thread state contradicts the actual worktree cleanup outcome.

## Why This Exists In Addition To The Other Live Scenarios

- `live-worker-direct-patch` proves one live worker turn can land a bounded fix.
- `live-supervisor-micro-proposal` proves a real report can drive a proposal and approval.
- `live-reject-redirect` proves operator redirect can create a corrected next assignment.
- `live-restart-resume` proves recovery after daemon interruption.
- This scenario proves the tracked-thread worktree lifecycle and cleanup path on a real live lane.

## Cleanup Expectations

This scenario expects the cleanup policy to remove the worktree after successful landing and prune. The final state should show the workspace as pruned, and the on-disk worktree path should no longer exist.

## Known Flake Points

- Live Codex latency can vary.
- The bootstrap worker turn can take a while before the thread id is available for binding.
- The tracked-thread lifecycle commands depend on the worktree having only the expected bounded change before each new step.
- The daemon bootstrap may take a few seconds before the first live turn starts.

## Why This Stops At Cleanup

The purpose of this slice is to prove the worktree lifecycle boundary:

1. tracked-thread worktree contract
2. bounded live fix in the lane
3. merge readiness
4. landing authorization and execution
5. workspace prune / cleanup

Executing another follow-on assignment would add scope without proving a new lifecycle boundary here.
