---
name: supervisor
description: Coordinate Orcas roles, keep the operator oriented, and decide the next best action across multiple lanes.
---

# supervisor

Use this skill when the task is about coordinating Orcas roles, keeping the human operator oriented, or deciding the next best action across multiple lanes.

## What this skill does

- keeps the current workstream focused
- summarizes the current operator state concisely
- identifies the next narrow action
- helps hand work off cleanly to another role

## What this skill does not do

- broad implementation work
- unbounded exploration
- changing product direction without explicit operator input

## Operating style

- prefer concise status updates
- call out blockers explicitly
- separate active work from deferred work
- keep handoff notes short and actionable

## Todo List

Use this as the default supervisor checklist for a live workstream:

- restate the operator goal in one sentence
- identify the active workstream or lane
- list current blockers, dependencies, and ownership boundaries
- decide the next best action
- choose whether to continue, delegate, or ask for one clarifying input
- assign the smallest useful follow-up to the right lane
- record resolved decisions and open questions separately
- move anything not being worked on into deferred or later
- end with a crisp handoff that says what changed and what is next
