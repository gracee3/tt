# Scenario: Live Multi-Phase Lane

## Goal

Prove that one tracked-thread worktree lane can survive multiple live review/approval steps without losing continuity.

This scenario uses one declared tracked-thread lane, two bounded live assignments, one approval-driven handoff between phases, and a final completion decision.

## What Is Seeded

1. A tiny git repository is materialized under the scenario worktree output root.
2. The repository contains a tiny C program, a shell test, and a Makefile with one obvious bug.
3. Orcas daemon state is started and one work unit is created in a workstream.
4. A tracked-thread record is created that points at the declared repository root, worktree path, branch name, base ref, landing target, strategy, sync policy, and cleanup policy before any live assignment begins.
5. The harness inspects the workstream runtime before phase 1 and expects the lane to be declared but not yet running as an active runtime thread.

## What Is Live

- Phase 1 is a real worker turn on the declared tracked-thread lane.
- Phase 1 report ingestion is real and produces a persisted report.
- Phase 1 supervisor review is real and creates a redirected next assignment on the same work unit.
- Phase 2 is a real worker turn that reuses the same tracked-thread/worktree lane.
- Phase 2 report ingestion is real and produces a second persisted report.
- Final completion is a real decision applied to the second report.
- The persisted tracked-thread, assignment, report, proposal, and decision records are all produced by Orcas.

## Live Boundary

- Before execution begins, the harness may create the tiny fixture, create the workstream and work unit, and create the tracked-thread record with its workspace contract.
- After the first live worker turn begins, the harness must not patch the source file, seed a report, seed a proposal, fake a tracked-thread binding update, or fake the second assignment.
- The harness may inspect CLI-visible state, request the next live phase through the supported proposal/decision path, and apply the final completion decision.

## What This Proves

- Orcas can use one declared tracked-thread lane across multiple live assignments.
- Orcas can keep that lane represented as exactly one managed workstream-runtime thread across both phases.
- Orcas can preserve the same tracked-thread id, repository root, worktree path, and branch identity across phases.
- Orcas can hand off from phase 1 to phase 2 through a real supervisor approval on the same work unit.
- Orcas can keep the lane bounded while letting the worker make two separate, inspectable changes on the same worktree.
- Orcas can complete the work unit cleanly after the second phase without creating a new lane.

## Pass Conditions

- The tracked-thread record shows the expected workspace contract before execution.
- The workstream runtime exists before phase 1 with zero active lane threads.
- Phase 1 completes and produces a persisted report.
- Phase 1 changes only `main.c` and `make test` passes.
- Phase 1 proposal approval creates the next assignment on the same work unit.
- The next assignment uses the same tracked-thread/worktree lane identity.
- The same managed workstream-runtime thread stays attached to that lane across both phases.
- Phase 2 completes and produces a persisted report.
- Phase 2 changes remain bounded and stay inside the declared worktree lane.
- The same tracked-thread id and worktree path are visible before phase 1, between phases, and after phase 2.
- The final completion decision marks the work unit completed.
- The final tracked-thread/workspace state remains consistent with the observed lane behavior.

## Fail Conditions

- The tracked-thread record does not show the expected workspace contract.
- The workstream runtime does not reflect the single-lane thread model before or during the two phases.
- Phase 1 never reaches a terminal persisted state.
- Phase 1 changes too many files or leaves the worktree outside the declared lane.
- The phase 1 approval does not create the next assignment on the same work unit.
- Phase 2 starts a different lane, creates a second worktree path, or requires a harness-side rebind.
- Phase 2 never reaches a terminal persisted state.
- The second phase changes are not bounded.
- The final completion decision fails.
- The final persisted tracked-thread state contradicts the observed lane continuity.

## Why This Exists In Addition To The Other Live Scenarios

- `live-worker-direct-patch` proves one real live worker turn can land a bounded fix.
- `live-supervisor-micro-proposal` proves a real report can drive a proposal and approval.
- `live-reject-redirect` proves operator redirect can create a corrected next assignment.
- `live-restart-resume` proves recovery after daemon interruption.
- `live-worktree-lifecycle` proves the tracked-thread worktree lifecycle and cleanup path.
- This scenario proves one tracked-thread/worktree lane can survive multiple live review/approval phases without losing continuity.

## Cleanup Expectations

This scenario intentionally stops at work-unit completion rather than pruning the worktree. The tracked-thread lane should remain inspectable after the second phase, and the final state should show the same declared lane still in place.

## Known Flake Points

- Live Codex latency can vary.
- The local supervisor model can take a while to generate each proposal.
- The tracked-thread lane depends on the worker staying within the declared worktree path and not creating backup or temp files.
- The phase 2 handoff depends on the phase 1 proposal creating the next assignment on the same work unit.

## Why This Stops At Completion

The purpose of this slice is to prove continuity across two live phases, not to prove pruning or workspace cleanup again. Once the second phase has completed and the work unit is closed, the scenario has validated the lane continuity boundary.
