# Orcas Control-Plane Alignment

## 1. Findings

- Orcas already has canonical authority records for `workstream`, `work_unit`, and `tracked_thread` in `state.db`.
- `tracked_thread.workspace` already models most of the intended workspace shape: repo root, worktree path, base ref, landing target, and lifecycle policy.
- Runtime state persists in SQLite runtime snapshots inside `state.db`: assignments, reports, worker sessions, workspace operations, thread mirrors, and landing/prune execution records.
- The daemon currently owns one global upstream Codex/app-server connection policy. There is no explicit workstream-scoped execution owner yet.
- Assignment packets already have an `execution_context`, but Orcas was not using it to start turns; `cwd` and `model` were being dropped at dispatch time.
- Worker-session reuse was broad. Reuse was keyed only by `worker_id`, which allowed session/thread reuse to bleed across unrelated work units.
- Git worktree inspection already existed for read-side status checks, but not for deriving the filesystem roots needed to express a future sandbox contract.

## 2. Gap Analysis

- Missing durable workstream execution scope: no place to persist `CODEX_HOME`, optional sqlite path, transport policy, or intended app-server ownership.
- Missing execution enforcement: workspace-aware `cwd` and requested `model` were not being forwarded into worker turns.
- Missing isolation boundary: worker-session reuse could cross work-unit or workspace boundaries.
- Missing read model: tracked-thread detail did not expose the concrete filesystem roots needed for workspace/thread-level sandboxing with git worktrees.
- Missing ownership clarity: Orcas had no explicit split between what it persists versus what remains app-server or Codex local state.

## 3. Recommended Architecture

- Keep Orcas as the supervisor and control plane.
- Treat one persistent app-server per workstream as the long-term ownership unit.
- Attach filesystem sandboxing to the workspace/thread, not to the workstream.
- Keep merge, prune, and landing authority in Orcas supervisor flows.
- Use `state.db` as the source of truth for workstream, work unit, tracked thread, workspace intent, and workstream execution scope.
- Keep runtime mirrors in SQLite runtime snapshots inside `state.db`.

## 4. Lifecycle

1. Create workstream.
   Persist canonical workstream metadata plus optional execution scope.
2. Start app-server.
   Resolve app-server startup, reconnect, and stop semantics from the workstream execution scope.
3. Create work unit.
   Persist the supervisor-managed task grouping under the workstream.
4. Allocate workspace/worktree.
   Persist workspace intent on the tracked-thread record and trigger explicit workspace operations.
5. Start Codex thread in that workspace.
   Resolve turn `cwd`, repo root, related git-admin roots, and requested model from the workspace and assignment contract.
6. Collect results.
   Continue using assignment communication packets plus reports, workspace operations, and landing/prune records.
7. Supervisor decides merge, prune, or follow-up.
   Orcas remains the only merge/prune authority.

## 5. State Model

Persist in Orcas:

- Workstream: title, objective, status, priority, execution scope.
- Work unit: task grouping and status.
- Workspace: tracked-thread workspace intent and lifecycle status.
- Codex thread binding: tracked-thread upstream binding, preferred `cwd`, preferred model, last seen turn.
- Worker runtime mirrors: assignments, worker sessions, reports, workspace operations, landing/prune execution state.

Delegate to Codex/app-server:

- Actual thread transcript/history storage.
- `CODEX_HOME` local state and sqlite-local state.
- Live thread runtime state outside Orcas mirrors.

Single source of truth:

- `state.db` for planning/control-plane authority and runtime mirrors.

## 6. Sandboxing / Git Worktree Constraints

Sandboxes should attach to the workspace/thread.

Normal worker turn roots:

- workspace worktree path
- worktree git dir from `git rev-parse --git-dir`
- shared common git dir from `git rev-parse --git-common-dir`

Workspace lifecycle roots for prepare, refresh, merge-prep, prune, and landing:

- all normal worker-turn roots
- repository root
- parent directory of the worktree path

Reasoning:

- reading and editing inside a worktree is not enough for git worktree-aware operations
- git needs access to the shared admin metadata under the common `.git`
- add/remove/prune flows also need the repo root and worktree parent path

Remote WebSocket transport exists, but it should remain non-default until authentication, endpoint ownership, and secret handling are hardened.

## 7. Implementation Plan

Phase 1:

- add workstream execution-scope fields to authority state and CLI
- expose tracked-thread filesystem scope in read models

Phase 2:

- resolve execution context from workspace intent
- pass `cwd` and model into turns
- tighten worker-session reuse to same work-unit lanes only

Phase 3:

- enforce per-workspace sandbox policy using the derived filesystem root contract
- route daemon app-server ownership by workstream execution scope
- add explicit workstream runtime lifecycle control at the Orcas RPC and CLI layer

## 8. Code Changes

Implemented in this slice:

- Added `WorkstreamExecutionScope` to authority workstreams, with transport, app-server, and connection-mode policy fields.
- Added SQLite schema migration and persistence for the workstream execution scope.
- Added tracked-thread filesystem scope reads, including worktree path, worktree parent, git dir, common git dir, worker-turn roots, and workspace-lifecycle roots.
- Changed assignment communication rendering to derive execution context from workspace intent.
- Changed turn dispatch to use the rendered assignment `cwd` and requested model instead of dropping both.
- Tightened worker-session reuse so reuse only happens within the same work-unit lane; otherwise Orcas allocates a fresh session.
- Added focused tests for execution-scope persistence, workspace filesystem-scope derivation, execution-context routing, and session reuse.
- Added explicit `workstream_runtime/{list,get,start,stop,restart}` RPCs and `orcas workstreams runtime ...` CLI commands.
- Added client disconnect and app-server stop hooks so Orcas can stop dedicated local runtimes and disconnect remote runtime clients.
- Scoped `threads/list` and `threads/list_loaded` to an explicit `workstream_id`.

## 9. Risks / Open Questions

- Orcas still uses one global upstream Codex/app-server client at runtime. This slice adds the control-plane model but does not yet switch to real per-workstream routing.
- Collaboration runtime state is still separate from authority state.
- Stale worktrees, abandoned sessions, and merge conflicts still require explicit supervisor handling; this slice improves visibility and isolation but does not automate cleanup policy.
- Remote WebSocket transport production posture still needs explicit security review.

## Out of Scope

- spawning and supervising dedicated per-workstream app-server processes
- moving collaboration runtime state into `state.db`
- changing supervisor-to-worker communication away from the existing assignment/report envelope flow

## Implementation Follow-Through

- Workstream-scoped model listing now routes through the selected workstream runtime instead of the shared upstream client.
- Worker thread start and resume now default to Codex `WorkspaceWrite` sandbox mode.
- Worker turn dispatch now derives `WorkspaceWrite` writable roots from tracked-thread workspace state:
  - normal worker turns use the worktree roots
  - workspace lifecycle turns expand to the repository root and worktree parent as well
