# Scenario: Git Worktrees

## Goal

Verify that Orcas can declare an isolated tracked-thread worktree, export scenario prompts to disk, and leave source code on disk inside the worktree.

## Steps

1. Create a tracked thread with an explicit workspace contract.
2. Export operator, supervisor, and agent prompt text files to disk.
3. Materialize the declared worktree path and write a small C program there.
4. Build and run the program from inside the worktree.
5. Inspect the tracked thread from the CLI and confirm the workspace state reflects the on-disk worktree.
6. Clean the repo with `make clean-e2e` to remove generated E2E worktrees, logs, and exports.

## Expected Result

- The tracked-thread workspace is declared and prompt text files are exported.
- The resulting code is visible on disk inside the worktree.
- All generated files live under `target/e2e/`.
