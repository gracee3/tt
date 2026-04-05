# Orcas Collaboration

## Overview

Orcas keeps supervision state local, but the current implementation has more than one live state surface. The daemon owns the local IPC contract, the live bridge to the upstream Codex app-server, legacy collaboration state, and the authority store. The CLI is a daemon client. The operator client reads and mutates supervised state through the daemon, while `orcas tui` provides the dashboard wrapper that can launch the real upstream `codex resume` TUI for a selected thread.

This document describes the current implemented contract rather than an aspirational target. It focuses on:

- source of truth for each major object class
- the difference between collaboration-owned state and authority-owned state
- merged read models such as `state/get`
- current mutation visibility and event behavior
- restart and reconnect expectations
- current operator surface splits between the CLI, the operator client, and the daemon

The local-authority rationale remains documented in [Local-Authority MVP Backend Design](design/local-authority-mvp-backend.md). The tracked-thread local binding decision remains documented in [ADR 0001](adr/0001-tracked-thread-is-a-local-binding-record.md).

## Collaboration Model

Orcas currently models work across one daemon-owned durable store plus one operator-client-local session surface.

- Authority state for authority workstreams, authority work units, and tracked threads lives in SQLite `state.db`.
- Collaboration/runtime state also persists in SQLite `state.db` as daemon-owned runtime snapshots and runtime-status rows.
- `state/get` is a merged derived snapshot that combines daemon state with collaboration-owned summaries plus any explicit authority compatibility bridge rows needed for assignment execution.
- `authority/hierarchy/get` is an authority-only hierarchy query over the SQLite store.
- The operator client also can launch a local `codex resume` TUI session for a selected thread, but that child process is not daemon-owned.

Tracked threads are Orcas-owned local binding records, not upstream Codex thread rows. A tracked thread may reference an `upstream_thread_id`, but create, edit, and delete operations act on the local Orcas record rather than claiming ownership of upstream runtime storage.

The important rule is that ORCAS does not currently present a single uniform workflow backend. Some operator-visible objects share a name while coming from different owners and read models.

## Surface Classification

ORCAS now treats public and internal surfaces as belonging to one of four buckets:

- Canonical:
  - authority-backed planning CRUD and planning reads
  - operator-facing CLI namespaces `orcas workstreams ...`, `orcas workunits ...`, and `orcas tracked-threads ...`
  - daemon authority planning RPCs such as `authority/hierarchy/get`, `authority/workstream/*`, `authority/workunit/*`, and `authority/tracked_thread/*`
- Runtime-detail exception:
  - `workunit/get`, which remains public because the operator client uses it for collaboration execution detail such as assignments, reports, decisions, and proposals
- Compatibility/internal:
  - collaboration-shaped bridge rows in `state/get`
  - collaboration planning mirrors persisted in SQLite runtime snapshots inside `state.db`
  - bridge tracking and projection logic inside the daemon that still supports execution flows
- Test-only:
  - `#[cfg(test)]` collaboration workstream/work-unit helpers in `orcasd/src/service.rs`

New planning features should land only on canonical surfaces. Runtime-detail exceptions, compatibility surfaces, and test-only helpers are not expansion targets for new operator-facing planning behavior.

## Workstream Plan

Orcas now persists a canonical plan per workstream in daemon collaboration state. The plan is not a per-work-unit authority; work units and assignments can reference it, but they do not own it.

The persisted plan records:

- a stable plan id and version
- workstream overview, goals, ordered plan items, success criteria, and constraints
- a supervisor execution policy (`Strict`, `Balanced`, or `Exploratory`)
- the current focus item for supervision
- assessments and revision proposals tied to that plan version

Supervisor execution may update plan-adjacent runtime fields without operator approval:

- assignment linkage to a plan item or a narrow special execution kind
- work item status updates such as `in_progress`, `blocked`, or `done`
- alignment and drift assessments
- notes, evidence refs, and recommended next focus within the current plan

Structural plan changes require operator approval before they become canonical. That includes adding or removing goals or items, changing ordering or priority, changing success criteria or constraints, and changing the exploration policy. The supervisor may propose such changes, but the daemon only applies them as a new plan version after approval.

When a supervisor proposes a revision, the daemon stores the proposal against the active plan version, validates it semantically against the current canonical plan, and preserves the prior version for historical inspection. Revision application now uses an explicit lifecycle: `pending`, `applying`, `applied`, `apply_failed`, `rejected`, or `superseded`. Orcas does not silently advance the canonical plan before downstream approval effects complete. If downstream apply fails, the revision remains inspectable with structured failure state instead of disappearing into logs.

`ApplyFailed` is now split into explicit recovery classes so operators can tell what kind of recovery is safe:

- `FailedBeforeDownstream` with `retry_safe = true` means the revision can be re-approved without duplicating downstream work.
- `FailedDuringDownstream` means downstream completion is uncertain; retry is blocked and the operator must inspect or intervene.
- `FailedAfterDownstream` with `reconcile_available = true` means downstream effects are believed complete, but plan finalization failed. The daemon can reconcile and finalize without re-running downstream work.

The daemon records whether downstream apply started, whether it completed, whether retry is safe, and whether operator intervention is required. Retry and reconcile are both explicit and state-gated; stale or superseded proposals still cannot be revived unsafely.

Assignment start and supervisor prompt generation both include plan linkage so tactical work stays anchored to the workstream plan rather than drifting into free-form local context. Direct execution assignments must resolve to a concrete `plan_item_id`. Narrow special execution kinds such as plan review or blocker investigation may omit a plan item, but they still attach to the active workstream plan version.

Runtime execution may only synchronize a narrow subset of canonical plan fields automatically:

- plan item status progression (`pending`, `in_progress`, `blocked`, `done`)
- linked evidence and linked assignment references
- current focus selection when execution clearly advances past a completed or dropped focus item

Runtime synchronization does not mutate plan structure, ordering, priorities, acceptance criteria, constraints, or exploration policy.

The review and collaboration operator surfaces now render plan revision recovery state directly from the canonical plan/proposal records. Failed revisions show the recovery phase, failure kind, retry safety, reconcile availability, operator-intervention requirement, and a short next-action hint so the operator does not need to infer recovery state from logs alone.

## Source-Of-Truth Matrix

### Ownership And Read Paths

| Object or state class | Authoritative owner | Durable persistence owner | Canonical mutation path | Canonical read path(s) | In `state/get` | In `authority/hierarchy/get` |
| --- | --- | --- | --- | --- | --- | --- |
| Workstream, legacy collaboration record | Daemon collaboration state | SQLite runtime snapshots in `state.db` | Internal daemon updates only | `state/get` | Yes | No |
| Workstream, authority record | Authority SQLite store | `state.db` | `authority/workstream/create`, `authority/workstream/edit`, `authority/workstream/delete` | `authority/hierarchy/get`, `authority/workstream/get`; `state/get` only after explicit assignment-compatibility bridging | Only if explicitly bridged as a collaboration-shaped summary with `source_kind = authority_compatibility_bridge` | Yes |
| Work unit, legacy collaboration record | Daemon collaboration state | SQLite runtime snapshots in `state.db` | Internal daemon updates only | `state/get`, `workunit/get` for execution detail | Yes | No |
| Work unit, authority record | Authority SQLite store | `state.db` | `authority/workunit/create`, `authority/workunit/edit`, `authority/workunit/delete` | `authority/hierarchy/get`, `authority/workunit/get`; `state/get` only after explicit assignment-compatibility bridging | Only if explicitly bridged as a collaboration-shaped summary with `source_kind = authority_compatibility_bridge` | Yes |
| Tracked thread, authority record | Authority SQLite store | `state.db` | `authority/tracked_thread/create`, `authority/tracked_thread/edit`, `authority/tracked_thread/delete` | `authority/hierarchy/get`, `authority/workunit/get`, `authority/tracked_thread/get` | No tracked-thread rows appear directly in `state/get` | Yes |
| Assignment | Daemon collaboration state | SQLite runtime snapshots in `state.db` | `assignment/start` plus daemon-owned lifecycle transitions | `state/get`, `assignment/get` | Yes | No |
| Proposal | Daemon collaboration state | SQLite runtime snapshots in `state.db` | `proposal/create`, `proposal/approve`, `proposal/reject`, plus daemon-owned generation and supersession | `proposal/get`, `proposal/list_for_workunit`, event stream, and nested proposal summary inside collaboration work unit summaries | No top-level proposal list in `state/get`; proposal summary can appear inside collaboration work unit summaries | No |
| Decision | Daemon collaboration state | SQLite runtime snapshots in `state.db` | `decision/apply` | `state/get`, `decision/apply` response | Yes | No |
| Report | Daemon collaboration state | SQLite runtime snapshots in `state.db` | Internal daemon recording during assignment and report handling | `state/get`, `report/get`, `report/list_for_workunit` | Yes | No |
| Worker session | Daemon collaboration state | SQLite runtime snapshots in `state.db` | Internal daemon-only selection and lifecycle updates | No dedicated public query; visible indirectly through assignment behavior and persisted collaboration state | No | No |
| Live thread state | Daemon live state mirrored from Codex | Thread mirror data in SQLite runtime snapshots in `state.db` | `thread/start`, `thread/resume`, daemon Codex event bridge, internal mirror maintenance | `state/get`, `threads/list*`, `thread/read*`, `thread/get` | Yes | No |
| Live turn state | Daemon live state mirrored from Codex | Turn mirror data in SQLite runtime snapshots in `state.db` | `turn/start`, `turn/steer`, `turn/interrupt`, daemon Codex event bridge, internal mirror maintenance | `state/get` active thread view, `turns/list_active`, `turns/recent`, `turn/get`, `turn/attach` | Yes, through session and active thread data | No |
| Codex resume child session state | operator-client-local `CodexSessionManager` | None | operator client `ResumeSelectedThreadInCodex` action and local child-process lifecycle | operator-client-local state only | No | No |

### Projection, Visibility, And Restart Behavior

| Object or state class | Projected, derived, or synthesized notes | Current event visibility | Restart and reconnect behavior |
| --- | --- | --- | --- |
| Workstream, legacy collaboration record | Collaboration-shaped native record | `WorkstreamLifecycle` for collaboration updates | Survives daemon restart through runtime snapshots in `state.db`; clients reload via snapshot-first flow |
| Workstream, authority record | Not globally projected into `state/get`. A workstream can appear there only as an explicit assignment-compatibility bridge row, and that row is still collaboration-shaped rather than authority-shaped. | `authority/workstream/create`, `authority/workstream/edit`, and `authority/workstream/delete` emit `WorkstreamLifecycle` with `created`, `updated`, and `deleted` actions | Survives daemon restart through `state.db`; old legacy imports can still leave hidden bridge copies in runtime snapshots, but `state/get` hides bridged rows whose authority source is tombstoned |
| Work unit, legacy collaboration record | Collaboration-shaped native record | `WorkUnitLifecycle` for collaboration updates | Survives daemon restart through runtime snapshots in `state.db`; clients reload via snapshot-first flow |
| Work unit, authority record | Not globally projected into `state/get`. It appears there only after assignment compatibility bridging has injected a collaboration-shaped row. | `authority/workunit/create`, `authority/workunit/edit`, and `authority/workunit/delete` emit `WorkUnitLifecycle` with `created`, `updated`, and `deleted` actions | Survives daemon restart through `state.db`; assignment-created collaboration compatibility state survives in runtime snapshots, but `state/get` hides bridged rows whose authority source is tombstoned |
| Tracked thread, authority record | Not projected into `state/get`; the operator client now reloads authority detail instead of synthesizing local authority records for edit flows | `authority/tracked_thread/create`, `authority/tracked_thread/edit`, and `authority/tracked_thread/delete` emit `TrackedThreadLifecycle` with `created`, `updated`, and `deleted` actions | Survives daemon restart through `state.db`; clients still reload authority hierarchy or detail queries when they need the current read model rather than just the lifecycle notification |
| Assignment | Collaboration-native | `AssignmentLifecycle` | Survives daemon restart through runtime snapshots in `state.db`; clients reload via snapshot-first flow |
| Proposal | Collaboration-native; there is no top-level proposal list in `state/get`, though collaboration work unit summaries can carry nested proposal summaries | `ProposalLifecycle` | Survives daemon restart through runtime snapshots in `state.db`; clients must re-query proposal RPCs after reconnect when they need full proposal records |
| Decision | Collaboration-native | `DecisionApplied` | Survives daemon restart through runtime snapshots in `state.db`; visible again through `state/get` |
| Report | Collaboration-native | `ReportRecorded` | Survives daemon restart through runtime snapshots in `state.db`; visible again through `state/get` and report RPCs |
| Worker session | Collaboration-native internal state | No dedicated worker-session event | Survives daemon restart through runtime snapshots in `state.db`; no dedicated client reload surface exists today |
| Live thread state | Derived from Codex plus daemon mirrors; not authority state | `UpstreamStatusChanged`, `SessionChanged`, `ThreadUpdated`, `TurnUpdated`, `ItemUpdated`, `OutputDelta` as applicable | Stored mirrors reload from runtime snapshots in `state.db`, but clients still treat reconnect as snapshot-first and `turn/attach` as daemon-instance scoped |
| Live turn state | Derived from Codex plus daemon mirrors | `TurnUpdated`, `ItemUpdated`, `OutputDelta`, `SessionChanged` as applicable | Stored mirrors reload from runtime snapshots in `state.db`, but attach and stream continuity are not promised across daemon restart |
| Codex resume child session state | operator client-only derived and runtime-managed; not reflected in daemon read models | No daemon event visibility | Does not survive operator client process exit or restart; daemon reconnect does not recreate it |

## IPC Contract

Orcas IPC uses a local Unix domain socket and JSON-RPC 2.0 style messages. Messages are newline-delimited JSON records. Clients issue requests for commands and queries, receive responses for results, and subscribe to notifications for incremental updates.

The daemon exposes a snapshot-first interaction pattern. Clients typically request current state first, then subscribe to live events. That keeps reconnect behavior deterministic and avoids rebuilding UI state from raw event gaps. The important caveat is that `state/get` is not the full authority hierarchy, and authority lifecycle events are visibility signals rather than full read-model convergence.

Current request families include:

- daemon lifecycle and status:
  - `daemon/status`
  - `daemon/connect`
  - `daemon/stop`
  - `daemon/disconnect`
- snapshot and session state:
  - `state/get`
  - `session/get_active`
- models and thread views:
  - `models/list` requires a target `workstream_id` and resolves through that workstream's runtime
  - `threads/list` requires a target `workstream_id` and resolves through that workstream's runtime
  - `threads/list_loaded` requires a target `workstream_id` and resolves through that workstream's runtime
  - thread summaries surface `management_state`, `owner_workstream_id`, and `runtime_workstream_id`
  - shared-runtime thread lists are owner-scoped: Orcas only returns threads it has explicitly bound to the requested workstream
  - dedicated-runtime thread lists can still surface externally created Codex threads as `observed_unmanaged` until Orcas explicitly adopts them into a managed lane
  - `threads/list_scoped` is deprecated
  - `thread/start`
  - `thread/read`
  - `thread/read_history`
  - `thread/get`
  - `thread/attach`
  - `thread/detach`
  - `thread/resume`
- workstream runtime control:
  - `workstream_runtime/list`
  - `workstream_runtime/get`
  - `workstream_runtime/start`
  - `workstream_runtime/stop`
  - `workstream_runtime/restart`
- turn views and turn control:
  - `turns/list_active`
  - `turns/recent`
  - `turn/get`
  - `turn/attach`
  - `turn/start`
  - `turn/steer`
  - `turn/interrupt`

Worker execution now defaults to Codex `WorkspaceWrite` sandboxing. Orcas applies thread-level `WorkspaceWrite` mode on worker thread start and resume, and derives turn-level writable roots from the tracked-thread workspace when a worker lane is bound to a git worktree.

Dedicated runtime stop and restart are conservative. Orcas refuses `workstream_runtime/stop` and `workstream_runtime/restart` when the runtime still reports any `observed_unmanaged` external threads, and idle-runtime retirement only stops a dedicated runtime when the runtime can be refreshed and reports zero observed threads. Shared-runtime workstream lists do not surface unowned external threads because they have no Orcas workstream owner.
- workflow and authority state:
  - `workunit/get`
  - `authority/hierarchy/get`
  - `authority/delete/plan`
  - `authority/workstream/create`
  - `authority/workstream/edit`
  - `authority/workstream/delete`
  - `authority/workstream/list`
  - `authority/workstream/get`
  - `authority/workunit/create`
  - `authority/workunit/edit`
  - `authority/workunit/delete`
  - `authority/workunit/list`
  - `authority/workunit/get`
  - `authority/tracked_thread/create`
  - `authority/tracked_thread/edit`
  - `authority/tracked_thread/delete`
  - `authority/tracked_thread/list`
  - `authority/tracked_thread/get`
  - `assignment/start`
  - `assignment/get`
  - `report/get`
  - `report/list_for_workunit`
  - `decision/apply`
- event subscription:
  - `events/subscribe`

Notifications are delivered on `events/notification` with Orcas-owned event envelopes. The daemon keeps a recent event buffer and bounded per-client queues so one slow frontend cannot stall the broker.

## Read-Model Contract

### `state/get`

`state/get` is the daemon's merged supervision snapshot. It currently contains:

- daemon status metadata
- active session state
- thread summaries and the active thread view
- a collaboration-shaped snapshot of workstreams, work units, assignments, codex thread assignments, supervisor decisions, reports, and decisions
- recent daemon event summaries

`state/get` is not a single-store source-of-truth dump. It is assembled from daemon memory plus any explicitly bridged authority workstream and work unit compatibility rows that already exist in collaboration state.

`state/get` does not contain:

- tracked-thread records
- authority revisions, tombstones, or origin-node metadata
- top-level proposal records, though work unit summaries can carry nested proposal summaries for collaboration-owned work units
- worker-session records
- operator-client-local Codex resume child session state

Current limitations of the merged collaboration snapshot:

- workstream and work unit lists can contain mixed semantics
- authority compatibility bridge rows appear as collaboration-shaped summaries rather than authority-shaped records
- bridge rows carry a narrower contract than authority hierarchy/detail reads; callers that need revisions, tombstones, origin metadata, tracked threads, or exact authority detail must use authority RPCs
- bridged authority rows now expose `source_kind = authority_compatibility_bridge` so clients can distinguish them from native collaboration rows
- authority deletes do not currently scrub previously bridged collaboration copies from runtime snapshots, but `state/get` hides those bridged rows once the authority source has been tombstoned
- later daemon-owned lifecycle updates on bridged rows, such as assignment-driven work-unit status changes, retain `source_kind = authority_compatibility_bridge` instead of masquerading as native collaboration rows

`source_kind` on planning summaries currently means:

- `collaboration`: collaboration-native summary owned by daemon collaboration state
- `authority_compatibility_bridge`: collaboration-shaped bridge summary kept only because execution state still depends on it
- `authority_projection`: authority lifecycle/event summary emitted from authority-owned planning state

Those labels are public provenance hints, not a promise that every Orcas surface exposes a fully normalized provenance model.

### `authority/hierarchy/get`

`authority/hierarchy/get` is the daemon's authority-only hierarchy query over SQLite. It returns authority workstreams, authority work units, and tracked threads using authority-shaped records and summaries.

This read model is the current source for:

- canonical planning hierarchy reads for authority workstreams, authority work units, and tracked threads
- tracked-thread hierarchy
- authority revisions
- authority tombstones when `include_deleted = true`
- authority-only metadata such as origin node identity

`authority/hierarchy/get` does not include:

- legacy collaboration-only workstreams or work units
- assignments
- proposals
- reports
- decisions
- worker sessions
- live thread or turn state

### Which Clients Rely On Which Read Models

- The CLI uses authority-backed planning CRUD for `orcas workstreams ...`, `orcas workunits ...`, and `orcas tracked-threads ...`, while still using `state/get` and focused RPCs for collaboration and runtime state.
- There is no longer an operator-facing legacy planning command namespace in the CLI.
- The operator client bootstraps from both `state/get` and `authority/hierarchy/get`.
- The operator client uses authority detail RPCs such as `authority/workstream/get`, `authority/workunit/get`, and `authority/tracked_thread/get` for focused editing surfaces.
- The operator client still uses `workunit/get` for collaboration execution detail on a selected work unit because that read includes assignments, reports, decisions, and proposals that are not part of the authority planning hierarchy.
- Existing subscribers should treat events as incremental hints layered on top of snapshot reloads, not as a complete replayable truth source for authority state.

### Current Client-Side Synthesis

The operator client no longer synthesizes authority-shaped edit records from hierarchy summaries.

- Edit forms now wait for `authority/workstream/get`, `authority/workunit/get`, or `authority/tracked_thread/get` to return.
- Hierarchy summaries are treated as navigation data, not as interchangeable authority detail records.

## Mutation And Event Visibility

### Authority-Owned Mutations

| Mutation | Durable write target | Read-after-write visibility | Event visibility today | What subscribers can rely on today |
| --- | --- | --- | --- | --- |
| `authority/workstream/create` | `state.db` | Appears in `authority/hierarchy/get` immediately after commit. It does not appear in `state/get` unless a later assignment compatibility bridge injects it into collaboration state. | Emits `WorkstreamLifecycle { action = created }` with `source_kind = authority_projection` | Subscribers can observe a post-commit create notification for that workstream id, but should treat the event as a visibility signal and reload authority reads for canonical data |
| `authority/workstream/edit` | `state.db` | Updated in `authority/hierarchy/get` after commit. Any existing bridge row is reflected in `state/get` on the next snapshot read because the bridge is stored in collaboration state. | Emits `WorkstreamLifecycle { action = updated }` with `source_kind = authority_projection` | Subscribers can observe a post-commit update notification for that workstream id, but should reload if they need authority revision, tombstone state, or exact authority detail |
| `authority/workstream/delete` | `state.db` tombstone | Hidden from default `authority/hierarchy/get`. `state/get` also hides any previously bridged collaboration copy on the next snapshot read, even though an imported bridge row can still remain in runtime snapshots. | Emits `WorkstreamLifecycle { action = deleted }` with `source_kind = authority_projection` | Subscribers can observe a post-commit delete notification for that workstream id. They must not infer that the hidden compatibility row was physically scrubbed. |
| `authority/workunit/create` | `state.db` | Appears in `authority/hierarchy/get` immediately after commit. It does not appear in `state/get` unless a later assignment compatibility bridge injects it into collaboration state. | Emits `WorkUnitLifecycle { action = created }` with `source_kind = authority_projection` | Subscribers can observe a post-commit create notification for that work unit id, but should reload authority reads for canonical planning data |
| `authority/workunit/edit` | `state.db` | Updated in `authority/hierarchy/get` after commit. Any existing bridge row is reflected in `state/get` on the next snapshot read because the bridge is stored in collaboration state. | Emits `WorkUnitLifecycle { action = updated }` with `source_kind = authority_projection` | Subscribers can observe a post-commit update notification for that work unit id, but should reload if they need the authority-shaped row |
| `authority/workunit/delete` | `state.db` tombstone | Hidden from default `authority/hierarchy/get`. `state/get` also hides any previously bridged collaboration copy on the next snapshot read, even though an imported bridge row can still remain in runtime snapshots. | Emits `WorkUnitLifecycle { action = deleted }` with `source_kind = authority_projection` | Subscribers can observe a post-commit delete notification for that work unit id. They must not infer that hidden compatibility rows were physically scrubbed. |
| `authority/tracked_thread/create` | `state.db` | Appears in `authority/hierarchy/get`, `authority/workunit/get`, and `authority/tracked_thread/get` after commit | Emits `TrackedThreadLifecycle { action = created }` | Subscribers can observe a post-commit create notification for that tracked thread id, but should reload authority queries if they need the full tracked-thread record |
| `authority/tracked_thread/edit` | `state.db` | Updated in authority detail and hierarchy queries after commit | Emits `TrackedThreadLifecycle { action = updated }` | Subscribers can observe a post-commit update notification for that tracked thread id, but should reload authority queries if they need full detail |
| `authority/tracked_thread/delete` | `state.db` tombstone | Hidden from default authority queries after commit | Emits `TrackedThreadLifecycle { action = deleted }` | Subscribers can observe a post-commit delete notification for that tracked thread id. They must not infer that every client-local cache has already been refreshed. |

Parent deletes also surface their explicit child tombstones through the daemon event stream:

- `authority/workstream/delete` emits the root `WorkstreamLifecycle { action = deleted }` event plus `WorkUnitLifecycle { action = deleted }` and `TrackedThreadLifecycle { action = deleted }` for cascaded descendants.
- `authority/workunit/delete` emits the root `WorkUnitLifecycle { action = deleted }` event plus `TrackedThreadLifecycle { action = deleted }` for cascaded descendants.

### Collaboration-Owned Mutations For Contrast

The daemon now emits post-commit lifecycle notifications for authority CRUD mutations, but event coverage is still object-specific rather than universal.

- Workstream and work unit lifecycle events exist for collaboration-owned records.
- Authority workstream and authority work unit mutations reuse those same lifecycle event families, with `created`, `updated`, and `deleted` action values.
- Assignments emit `AssignmentLifecycle`.
- Codex thread assignments emit `CodexAssignmentLifecycle`.
- Supervisor turn decisions emit `SupervisorDecisionLifecycle`.
- Reports emit `ReportRecorded`.
- Decisions emit `DecisionApplied`.
- Proposals emit `ProposalLifecycle`.
- Authority tracked-thread mutations emit `TrackedThreadLifecycle`.
- Worker sessions do not have a dedicated daemon event family.

## Snapshot, Restart, And Reconnect Flow

The current client reconnect flow is:

1. Connect to the daemon socket.
2. Request `state/get`.
3. Request focused reads as needed.
4. Subscribe to `events/subscribe`.
5. If the socket drops or the daemon restarts, treat the old subscription as closed.
6. Reconnect, request fresh reads again, and only then consume new incremental events.

This remains the recommended flow for both the CLI and the operator client. The operator client adds an authority reload step because its main hierarchy depends on both `state/get` and `authority/hierarchy/get`.

There is still no replay contract for missed daemon events. A closed or interrupted subscription means the client must re-read current state rather than infer what happened while it was disconnected.

### Persistence Notes

- `state.db` remains the only durable Orcas store. The authority SQLite store persists authority workstreams, authority work units, tracked threads, revisions, tombstones, command receipts, authority event history, runtime snapshots, and workstream runtime rows.
- A legacy `state.json` may still be present only as one-time import input. Orcas does not depend on it after import completes.
- On first authority-store initialization, SQLite can bootstrap from existing `state.json` if authority or runtime snapshot data has not already been recorded in `state.db`.

### What Survives Daemon Restart

- collaboration state in runtime snapshots in `state.db`
- thread and turn mirror state in runtime snapshots in `state.db`
- authority state in `state.db`

### What Clients Must Reload

- `state/get` after reconnect
- `authority/hierarchy/get` for operator client hierarchy views after reconnect
- authority detail queries when exact authority fields are needed
- proposal and other focused RPCs for data that is not included in `state/get`

The current operator client reconnect path also treats authority-only view state as invalidated until those reloads complete:

- authority hierarchy rows are cleared when the daemon connection is lost
- cached authority detail records are cleared when the daemon connection is lost
- the main-footer edit or delete mode resets to inspect on disconnect or fresh snapshot load
- the current authority selection is preserved only as selection intent and is restored after a fresh hierarchy reload if that row still exists

### operator client PTY Exception

The operator-client-local `codex resume` child session manager is not part of daemon state. It does not live in `state/get` or `authority/hierarchy/get`, and it is not reconstructed by daemon reconnect.

- If the daemon restarts, the operator client must reconnect and reload daemon-owned state separately.
- If the operator client process stays alive, an already-running local Codex resume child can continue independently while the daemon reconnects because that child session is operator client-owned rather than daemon-owned.
- If the operator client exits, the local Codex resume child exits with it unless the operator detached it separately.
- Local Codex child-session state should therefore be treated as operator convenience state rather than durable workflow state.

## Upstream Codex Integration

`orcasd` owns the upstream Codex app-server connection. Clients do not use the upstream WebSocket protocol directly for supervised-state reads or writes.

The daemon connects to a configured WebSocket endpoint, with a localhost endpoint used by default in the current configuration. The upstream transport details remain an internal implementation concern. Orcas surfaces the resulting thread, turn, collaboration, and authority query state through its own IPC contract instead of mirroring the upstream wire format wholesale.

The one intentional exception is the operator client's local `codex resume` child TUI. `orcas tui` may launch and manage those child processes interactively through a blank-first dashboard wrapper with a border HUD, but it remains an operator-owned convenience layer rather than a daemon-owned source of supervision truth. It no longer fetches collaboration workstream/thread sidebars on startup.

## Operator And Client Surfaces

- The canonical operator CRUD surface for planning hierarchy objects is now authority-backed in both clients:
  - CLI: `orcas workstreams ...`, `orcas workunits ...`, and `orcas tracked-threads ...`
  - operator client: authority CRUD plus `authority/hierarchy/get` and authority detail RPCs
- There is no longer an operator-facing legacy planning command namespace. Imported collaboration planning rows can still exist in runtime snapshots, but they are no longer exposed as a peer CLI surface.
- Both clients still depend on daemon snapshots and focused daemon RPCs for thread, turn, assignment, report, decision, and proposal views.
- The operator client retains one collaboration work-unit detail read, `workunit/get`, because it carries execution detail that is outside the authority planning hierarchy.
- Non-canonical surfaces are intentionally frozen: new planning behavior should not be added to `workunit/get`, bridge rows, or other collaboration compatibility paths.
- The daemon event stream is shared and now carries post-commit create, update, and delete notifications for authority workstreams, work units, and tracked threads.
- The operator client's `codex resume` child process is local to the operator client process and should be understood as an operator convenience layer rather than a daemon-managed session model.

## Known Limitations Carried Into Later Phases

- `state/get` still contains mixed-semantics workstream and work unit lists because collaboration-native rows and explicit compatibility bridge rows share summary types.
- Authority deletes hide previously bridged rows from `state/get`, but do not currently scrub the underlying imported bridge copy from runtime snapshots.
- Tracked-thread lifecycle events do not remove the need for authority query reloads when a client needs full record detail rather than an event summary.
- The daemon still carries one collaboration planning-era public read, `workunit/get`, because the operator client uses it for execution detail that is not modeled by authority planning reads. The rest of the old `workstream/*` and `workunit/*` planning CRUD/list surface has been retired from the public daemon contract.

These are current implementation truths, not guarantees that later hardening phases will preserve unchanged. Later phases can normalize the boundary, but this document intentionally describes the boundary as it exists today.
