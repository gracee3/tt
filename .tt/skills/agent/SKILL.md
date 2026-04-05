---
name: agent
description: Subagent spawning and lane coordination.
---

# agent

Use this skill when the task is to spawn, coordinate, inspect, or retire TT subagents.

## Scope

- owns subagent lifecycle for bounded tasks
- coordinates with `direct` as the parent playbook lane
- preserves the orchestration trail between parent and child turns
- keeps ownership boundaries explicit
- does not widen task scope on its own

## Discovery

- find the parent `direct` context and the active workstream
- identify the target repo, worktree, branch, and task boundary
- determine whether the child should run locally, remotely, or as a shared app-server backed TT session
- verify what state already exists before spawning another agent

## Tools

- `tt --remote ...` for remote TT sessions when the task belongs in the shared runtime
- app-server/session helpers for startup, resume, and lifecycle checks
- worktree and branch helpers when the task needs isolated repo state
- repo and status inspection commands before and after delegation

## Runtime State

- parent lane and turn intent
- child session or thread id
- repo root, worktree path, and branch
- whether the child is active, paused, completed, or blocked
- what artifacts the child produced and where they were written

## Protocol

- receive a bounded task from `direct`
- spawn or resume only the child needed for that task
- pass through the smallest complete set of instructions and context
- do not reassign the same work to multiple children unless the task is truly parallel
- return status, output, and next-step recommendations to `direct`

## Finish

- report what ran and where
- report whether the child is still live or has been retired
- report what changed, what remains open, and the next gate
- leave the parent thread ready for the next decision

## Failure Modes

- if the child cannot be spawned, report the exact blocker and the smallest recovery step
- if the session exists but is unhealthy, say whether it should be resumed, replaced, or cleaned up
- if the task is not suitably bounded, hand it back to `direct` instead of guessing
- if the runtime state conflicts, do not invent a fix; report the contradiction plainly
