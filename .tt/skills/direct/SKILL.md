---
name: direct
description: Main TT playbook for operator intake, todo tracking, and capability dispatch.
---

# direct

Use this skill when the task is the main operator playbook or runbook for a spawned TT lane.

## What this skill does

- receives user notes, goals, and interruptions
- maintains the shared backlog in `docs/WORKSTREAM_TODO.md`
- routes work to the right TT mode and capability skill
- keeps the thread ready for handoff
- preserves the smallest useful next action

## What this skill does not do

- broad implementation work by default
- unbounded exploration
- drifting past the current turn's scope

## Operating style

- restate the goal before acting
- keep active work separate from deferred work
- make ownership boundaries explicit
- ask only for the smallest missing fact
- end with a crisp handoff

## Todo

- insert: capture pasted notes, bugs, and context in the tracked backlog
- review: surface missing requirements, undefined edges, and open questions
- plan: turn the backlog into a decision-complete implementation plan after recon

## Tool routing

- `request_user_input`: the default clarification tool when one narrow missing fact blocks progress
- `update_plan`: structured planning output for `todo plan`
- `tool_search` / `tool_suggest` / `list_dir`: recon and discovery before implementation
- `apply_patch`: preferred edit primitive when the next step is a write-capable change
- `shell` / `write_stdin`: execution and long-running command control
- `spawn_agent` / `send_input` / `wait_agent` / `resume_agent` / `close_agent`: bounded delegation through the `agent` capability

## Capability map

- `todo`: notes, review, and planning for the tracked backlog
- `develop`: implementation and code changes
- `test`: validation, harness, and evidence capture
- `integrate`: branch and merge management
- `chat`: discussion-only handoff and summary
- `learn`: repo recon, reading, and gap-finding
- `handoff`: transfer packaging and ownership changeover
- `diff`: worktree review before cleanup
- `agent`: subagent spawning and lane coordination
- `process`: process lifecycle management and long-lived process state
- `i3`: window-manager coordination
- `tt`: TT session and shared app-server coordination
- `doctor`: diagnosis and failure localization
- `git`: branch, worktree, and repo-state coordination
- `human`: operator liaison and requirement clarification
- `clean`: cleanup, hygiene, and stale-artifact removal
- `services`: background service lifecycle coordination

## Preferred delegates

- use `agent` for subagent spawning and turn handoff
- use `i3` for desktop/window-manager coordination
- use `tt` for TT session and shared app-server lifecycle work

## Runtime surfaces

- use `tt skill process ...` for local process lifecycle control
- use `tt skill services ...` for daemon and shared app-server services
- use `tt skill git ...` for branch and worktree coordination
- use `tt skill i3 ...` for workspace and window-manager actions
