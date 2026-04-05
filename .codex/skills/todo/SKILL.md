---
name: todo
description: Maintain the tracked project todo file, normalize pasted notes, and grow them into a plan when ready.
---

# todo

Use this skill when the task is to ingest notes, keep a project backlog current, or turn scattered text into a tracked todo list.

## What this skill does

- maintains the shared backlog in `docs/WORKSTREAM_TODO.md`
- preserves pasted notes, bugs, requirements, and small follow-up questions
- groups related items and keeps the list readable
- moves from raw insertions to review and then to planning
- keeps the backlog ready for the next execution turn

## What this skill does not do

- broad implementation work
- unbounded exploration
- solutioning before the backlog is ready for it

## Operating style

- prefer short, incremental updates
- keep details that may be needed to recreate the issue
- separate active work from deferred items
- ask the minimum follow-up needed to close gaps
- keep the backlog as the source of truth across exchanges

## Modes

### Insert

- ingest pasted notes verbatim when useful
- rephrase only when it improves clarity or structure
- preserve bug reports, edge cases, constraints, and context
- update `docs/WORKSTREAM_TODO.md` directly
- return a concise changelog of what was added or reorganized
- ask for the next batch of text

### Review

- inspect the backlog for missing requirements and undefined edges
- prompt the user only for the smallest missing facts
- keep the user oriented to the current thread state
- allow user steering, but keep the backlog moving back toward completion

### Plan

- use repo context and source inspection to form an implementation plan
- separate proven facts from inferred next steps
- keep the plan concise and actionable
- let the user view and revise the plan as the work evolves

