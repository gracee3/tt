# Scenario: Git Worktrees

## Goal

Verify that Orcas can declare a tracked-thread worktree lane, bind it on the first live assignment, export operator-facing prompt artifacts, and carry that same lane through the tracked-thread workspace lifecycle until cleanup.

## What Is Seeded

1. The harness creates an isolated git repository and a dedicated worktree under `target/e2e/worktrees/...`.
2. The workstream, work unit, and tracked-thread workspace contract are created before any live assignment starts.
3. Operator, supervisor, and agent prompt text files are exported under `target/e2e/artifacts/...`.
4. The harness inspects the workstream runtime before execution and expects zero active managed lane threads.

## What Is Live

- The first worker turn is real and creates the tiny C project in the declared tracked-thread worktree lane.
- The tracked-thread binds automatically to the live upstream thread on that first assignment.
- The tracked-thread workspace lifecycle steps are real:
  - `prepare-workspace`
  - `refresh-workspace`
  - `merge-prep`
  - `authorize-merge`
  - `execute-landing`
  - `prune-workspace`
- The reopen/close decisions between lifecycle phases are real persisted operator decisions.

## Pass Conditions

- The tracked-thread workspace contract exists before the first live assignment.
- The workstream runtime exists before execution with zero managed lane threads.
- The first live assignment creates the tiny project inside the declared worktree and `make test` passes there.
- The tracked-thread auto-binds to the live upstream thread without a harness-side repair step.
- The runtime shows exactly one managed lane thread and no unmanaged thread leak after the first assignment.
- Prompt artifacts remain exported on disk for inspection.
- The lifecycle steps produce persisted records that match the observed worktree state through landing and prune.
- A final `MarkComplete` decision closes the work unit after lifecycle cleanup.

## Fail Conditions

- The tracked-thread workspace contract is missing or does not match the declared path.
- The runtime does not show the expected workstream/lane state before or after live execution.
- The first live assignment writes files outside the declared worktree or fails to leave a buildable tiny project.
- The tracked-thread requires a manual binding repair.
- Any tracked-thread workspace lifecycle step fails to record an honest result.
- Final persisted state contradicts the actual landing or prune outcome.
