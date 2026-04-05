---
name: codex
description: Codex session and shared app-server coordination.
---

# codex

Use this skill when the task is to spawn, inspect, resume, or maintain Codex sessions and the shared app-server they depend on.

## What this skill does

- coordinates Codex session lifecycle
- keeps the session and app-server state aligned
- handles startup, resumption, and teardown intent
- preserves the operator-facing context needed to continue work

## What this skill does not do

- unrelated repository work
- app logic changes outside session/runtime coordination
- broad process management that belongs in a more general lane
