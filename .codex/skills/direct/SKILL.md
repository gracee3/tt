---
name: direct
description: Main Codex playbook for operator intake, todo tracking, and capability dispatch.
---

# direct

Use this skill when the task is the main operator playbook or runbook for a spawned Codex lane.

## What this skill does

- receives user notes, goals, and interruptions
- maintains the shared backlog in `docs/WORKSTREAM_TODO.md`
- routes work to the right capability skill
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

## Capability map

- `chat`: human-facing conversation, summary, and handoff
- `learn`: repo recon, reading, and gap-finding
- `propose`: decision drafting and structured recommendations
- `process`: process lifecycle management and long-lived process state
- `i3`: window-manager coordination
- `codex`: Codex session and shared app-server coordination
- `doctor`: diagnosis and failure localization
- `git`: branch, worktree, and repo-state coordination
- `test`: validation, harness, and evidence capture
- `agent`: subagent spawning and lane coordination
- `human`: operator liaison and requirement clarification
- `clean`: cleanup, hygiene, and stale-artifact removal
- `services`: background service lifecycle coordination
