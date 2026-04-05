---
name: git
description: Branch, worktree, and repository-state coordination.
---

# git

Use this skill when the task is about git hygiene, branches, worktrees, or landing preparation.

## What this skill does

- keeps repository state clear and recoverable
- coordinates branch and worktree intent
- prepares commits and landing steps
- preserves a clean handoff trail

## Runtime surface

Prefer the typed `tt skill git ...` entrypoint:

- `tt skill git status [--repo-root <path>] [--worktree-path <path>]`
- `tt skill git branch current [--repo-root <path>]`
- `tt skill git branch list [--repo-root <path>]`
- `tt skill git worktree current [--repo-root <path>] [--worktree-path <path>]`
- `tt skill git worktree list [--repo-root <path>] [--worktree-path <path>]`

## What this skill does not do

- unrelated code changes
- merge policy changes without instruction
- risky git operations without a clear need
