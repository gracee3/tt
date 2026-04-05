---
name: codex
description: Codex session and shared app-server coordination.
---

# codex

Use this skill when the task is to spawn, inspect, resume, or maintain Codex sessions and the shared app-server they depend on.

## Scope

- owns Codex session lifecycle and shared app-server coordination
- keeps the session, remote endpoint, and app-server state aligned
- treats the session runtime as an operational dependency of the current lane
- does not replace the parent `direct` playbook

## Discovery

- identify the active Codex session or thread
- determine whether a shared app-server is already running
- inspect the remote endpoint, auth state, and session attachment state
- check whether the current work belongs to a local session or a remote one

## Tools

- `codex --remote ...` for remote session access and resume flows
- app-server start/inspect/resume helpers when the runtime is managed centrally
- session status commands before and after coordination actions
- cleanup helpers when a session should be retired rather than resumed

## Runtime State

- session id and thread id
- remote URL or server endpoint
- whether the session is attached, detached, running, or stopped
- which workspace or worktree the session is supposed to represent
- whether the session can safely be resumed without rework

## Protocol

- receive a session/runtime intent from `direct`
- choose the narrowest action that satisfies it
- keep app-server management subordinate to the current Codex session goal
- preserve any operator-facing context needed to continue the turn

## Finish

- confirm whether the session is ready, resumed, stopped, or intentionally left running
- report the endpoint or session identity involved
- leave a clear next step for the parent lane

## Failure Modes

- if the app-server is missing or unhealthy, report the exact condition and the smallest fix
- if the session state is inconsistent, do not invent a recovery path
- if the requested action would disrupt unrelated sessions, stop and surface the risk
- if the target runtime cannot be reached, return the blocker in operational terms
