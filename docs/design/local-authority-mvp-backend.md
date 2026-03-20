# Local-Authority MVP Backend Design

## 1. Executive Summary

This design makes `orcasd` the explicit local authority for Orcas state in the MVP. The daemon becomes the only write path for mutable operator objects, backed by a small SQLite store with a canonical event log and a set of query projections that directly power the TUI.

The recommended MVP keeps the architecture intentionally disciplined:

- local-only and fully offline
- Rust-first
- SQLite-backed
- explicit commands and events
- thin TUI over daemon IPC
- single-writer mutation path
- tombstone-based deletes
- future-ready seams for later replication without turning the local core into a fake cloud system now

The key semantic decision is that Orcas should persist an Orcas-owned `tracked_thread` object, not treat upstream Codex runtime thread rows as directly owned mutable state. A tracked thread is a durable local record under a work unit that may reference an upstream runtime thread, but deleting it only removes Orcas tracking locally. It does not promise hard deletion of the upstream runtime thread.

This document is design rationale for the MVP rather than a current protocol reference. Later hardening phases retired most of the public legacy collaboration planning RPC family discussed below; for the current implemented contract, use [Collaboration](../collaboration.md) and [Architecture](../architecture.md).

## 2. Recommended Architecture For Local-Authority MVP

### Decision

Use SQLite as the local durable store. Treat the SQLite event log as the canonical write history. Maintain current-state projection tables in the same database for fast TUI reads.

### Practical Shape

- `orcas-core`
  - domain IDs
  - command and event enums
  - projection/query DTOs
  - store traits that do not assume SQLite outside the trait boundary
- `orcasd`
  - SQLite-backed store implementation
  - command handlers
  - projector
  - JSON-RPC handlers
  - event notification fanout after commit
- `orcas-tui`
  - query-driven UI
  - generic footer/composer state for create, edit, and delete confirmation
  - no direct row mutation logic

### Mutation Discipline

All mutations should pass through one daemon-owned write executor. In practice, the MVP can use a single Tokio task or a single mutex-guarded SQLite connection for writes. The point is not theoretical purity. The point is that accepted commands produce exactly one ordered event stream and one consistent set of projections.

### SQLite Configuration

Recommended initial pragmas:

- `journal_mode = WAL`
- `synchronous = FULL`
- `foreign_keys = ON`
- `busy_timeout = 5000`

This keeps the store durable and portable while avoiding a heavy database layer.

### Event-Sourced, But Lightweight

Yes, the event log should be the canonical write history, with projections as read models. For Orcas MVP this can stay small:

- one `event_log` table
- one `command_receipts` table for idempotency
- one small set of current-state projection tables
- one projector path in the daemon

No distributed event bus is needed. No generic event framework is needed. SQLite transactions are enough.

## 3. Domain Model

### Source Of Truth

For MVP, the local daemon and its SQLite store are authoritative for Orcas-owned state.

### Entity Model

The initial mutable entities are:

1. `workstream`
2. `work_unit`
3. `tracked_thread`

### Common Fields

Each entity should carry:

- `id`
- `revision`
- `created_at`
- `updated_at`
- `deleted_at` nullable
- `origin_node_id`

### Workstream

Purpose: top-level operator grouping.

Suggested fields:

- `id`
- `title`
- `objective`
- `status`
- `priority`
- `revision`
- timestamps
- tombstone fields

### Work Unit

Purpose: concrete task under a workstream.

Suggested fields:

- `id`
- `workstream_id`
- `title`
- `task_statement`
- `status`
- `revision`
- timestamps
- tombstone fields

`dependencies` can remain deferred for this MVP backend pass unless the current collaboration surface requires preserving them during migration.

### Tracked Thread

Purpose: Orcas-owned durable record that tracks a thread-like execution lane under a work unit.

Suggested fields:

- `id`
- `work_unit_id`
- `title`
- `notes`
- `backend_kind` such as `codex`
- `upstream_thread_id` nullable
- `binding_state` such as `unbound`, `bound`, `detached`, `missing`
- `preferred_cwd` nullable
- `preferred_model` nullable
- `last_seen_turn_id` nullable
- `revision`
- timestamps
- tombstone fields

### Explicit Thread Semantics Decision

For MVP, Orcas should persist a `tracked_thread` object locally. It should not model upstream Codex thread rows as Orcas-owned mutable entities.

This resolves the semantic question as follows:

- create:
  - create a new Orcas tracking record under a work unit
  - it may start unbound
  - it may optionally bind to an existing upstream `thread_id`
- edit:
  - edit Orcas-owned metadata and binding fields
  - examples: title, notes, preferred cwd, preferred model, local association to a different upstream thread if explicitly allowed
- delete:
  - tombstone the Orcas tracking record locally
  - remove it from normal TUI hierarchy queries
  - never imply upstream hard deletion

This is the right MVP contract because Orcas does control local supervision records, but it does not control the full lifecycle guarantees of upstream runtime storage. The tracked-thread model preserves a clean future seam:

- today: Orcas owns local tracking
- later: Orcas can replicate tracked-thread records and reconcile them with cloud-authoritative runtime objects

Product wording should prefer `tracked thread`. Internally and in architecture language it is a local binding record to upstream runtime state when an upstream reference exists.

## 4. Command Catalog

Every mutation should be an explicit command. No direct row updates from the TUI.

### Command Envelope

Each command should carry:

- `command_id`
- `issued_at`
- `origin_node_id`
- `actor` such as `tui_operator`
- `correlation_id` optional
- `expected_revision` for update and delete

### Commands

#### Workstream

- `CreateWorkstream`
  - fields: `workstream_id`, `title`, `objective`, `status`, `priority`
- `EditWorkstream`
  - fields: `workstream_id`, `expected_revision`, changed attributes
- `DeleteWorkstream`
  - fields: `workstream_id`, `expected_revision`, `delete_token`

#### Work Unit

- `CreateWorkUnit`
  - fields: `work_unit_id`, `workstream_id`, `title`, `task_statement`, `status`
- `EditWorkUnit`
  - fields: `work_unit_id`, `expected_revision`, changed attributes
- `DeleteWorkUnit`
  - fields: `work_unit_id`, `expected_revision`, `delete_token`

#### Tracked Thread

- `CreateTrackedThread`
  - fields: `tracked_thread_id`, `work_unit_id`, `title`, `notes`, `backend_kind`, `upstream_thread_id`, `preferred_cwd`, `preferred_model`
- `EditTrackedThread`
  - fields: `tracked_thread_id`, `expected_revision`, changed attributes
- `DeleteTrackedThread`
  - fields: `tracked_thread_id`, `expected_revision`, `delete_token`

### Command Mapping From TUI

- footer create workstream -> `CreateWorkstream`
- footer edit workstream -> `EditWorkstream`
- footer delete workstream -> `DeleteWorkstream`
- footer create work unit under selected workstream -> `CreateWorkUnit`
- footer edit work unit -> `EditWorkUnit`
- footer delete work unit -> `DeleteWorkUnit`
- footer create tracked thread under selected work unit -> `CreateTrackedThread`
- footer edit tracked thread -> `EditTrackedThread`
- footer delete tracked thread -> `DeleteTrackedThread`

## 5. Event Catalog

Events are the canonical durable history.

### Event Envelope

Each event should carry:

- `event_id`
- `command_id`
- `aggregate_type`
- `aggregate_id`
- `aggregate_version`
- `occurred_at`
- `origin_node_id`
- `causation_id` optional
- `correlation_id` optional

### Entity Events

#### Workstream

- `WorkstreamCreated`
- `WorkstreamEdited`
- `WorkstreamDeleted`

#### Work Unit

- `WorkUnitCreated`
- `WorkUnitEdited`
- `WorkUnitDeleted`

#### Tracked Thread

- `TrackedThreadCreated`
- `TrackedThreadEdited`
- `TrackedThreadDeleted`

### Cascade Delete Event Rule

Parent deletes should materialize as explicit child tombstone events in the same transaction.

Example:

- deleting a workstream with two work units and three tracked threads writes:
  - `WorkstreamDeleted`
  - `WorkUnitDeleted` for each child work unit
  - `TrackedThreadDeleted` for each descendant tracked thread

This keeps replay simple. A fresh projector can rebuild current state from the event log without hidden delete side effects.

## 6. SQLite Schema Sketch

This is a recommended MVP schema sketch, not production DDL.

```sql
create table store_meta (
  key text primary key,
  value text not null
);

create table command_receipts (
  command_id text primary key,
  command_type text not null,
  aggregate_type text not null,
  aggregate_id text not null,
  accepted integer not null,
  response_json text,
  recorded_at text not null
);

create table event_log (
  seq integer primary key autoincrement,
  event_id text not null unique,
  command_id text not null,
  aggregate_type text not null,
  aggregate_id text not null,
  aggregate_version integer not null,
  event_type text not null,
  occurred_at text not null,
  origin_node_id text not null,
  causation_id text,
  correlation_id text,
  body_json text not null
);

create index idx_event_log_aggregate
  on event_log (aggregate_type, aggregate_id, aggregate_version);

create index idx_event_log_command
  on event_log (command_id);

create table projection_checkpoint (
  projection_name text primary key,
  last_applied_seq integer not null
);

create table workstreams (
  id text primary key,
  title text not null,
  objective text not null,
  status text not null,
  priority text not null,
  revision integer not null,
  origin_node_id text not null,
  created_at text not null,
  updated_at text not null,
  deleted_at text
);

create table work_units (
  id text primary key,
  workstream_id text not null,
  title text not null,
  task_statement text not null,
  status text not null,
  revision integer not null,
  origin_node_id text not null,
  created_at text not null,
  updated_at text not null,
  deleted_at text,
  foreign key (workstream_id) references workstreams(id)
);

create index idx_work_units_workstream
  on work_units (workstream_id, deleted_at, updated_at);

create table tracked_threads (
  id text primary key,
  work_unit_id text not null,
  title text not null,
  notes text,
  backend_kind text not null,
  upstream_thread_id text,
  binding_state text not null,
  preferred_cwd text,
  preferred_model text,
  last_seen_turn_id text,
  revision integer not null,
  origin_node_id text not null,
  created_at text not null,
  updated_at text not null,
  deleted_at text,
  foreign key (work_unit_id) references work_units(id)
);

create index idx_tracked_threads_work_unit
  on tracked_threads (work_unit_id, deleted_at, updated_at);

create unique index idx_tracked_threads_upstream_active
  on tracked_threads (upstream_thread_id)
  where upstream_thread_id is not null and deleted_at is null;

create view tui_hierarchy as
select
  ws.id as workstream_id,
  ws.title as workstream_title,
  ws.status as workstream_status,
  wu.id as work_unit_id,
  wu.title as work_unit_title,
  wu.status as work_unit_status,
  tt.id as tracked_thread_id,
  tt.title as tracked_thread_title,
  tt.binding_state as tracked_thread_binding_state,
  tt.upstream_thread_id as upstream_thread_id
from workstreams ws
left join work_units wu
  on wu.workstream_id = ws.id and wu.deleted_at is null
left join tracked_threads tt
  on tt.work_unit_id = wu.id and tt.deleted_at is null
where ws.deleted_at is null;
```

### MVP Tables That Should Exist

The likely MVP set is:

- `store_meta`
- `command_receipts`
- `event_log`
- `projection_checkpoint`
- `workstreams`
- `work_units`
- `tracked_threads`
- `tui_hierarchy` view

That is enough for the local-authority MVP.

### Identity

Use stable globally unique IDs from day one.

Recommendation:

- entity IDs: UUIDv7 strings
- command IDs: UUIDv7 strings
- event IDs: UUIDv7 strings
- `origin_node_id`: stable UUID generated once per local Orcas installation and stored in `store_meta`

UUIDv7 is preferred over UUIDv4 because it keeps ordering saner in SQLite and later sync logs.

## 7. Projection And Query Model

### Projection Ownership

Projection tables should be owned by the daemon store layer, not by the TUI.

The TUI should read:

- hierarchy lists from `workstreams`, `work_units`, `tracked_threads`, or `tui_hierarchy`
- detail surfaces from entity projection tables
- delete previews from query helpers that count descendants from the same projection tables

### Query Surfaces Needed For MVP

#### Hierarchy

- workstream list with counts
- work unit list for a workstream
- tracked thread list for a work unit
- combined hierarchy snapshot for fast TUI bootstrap

#### Detail

- workstream detail
- work unit detail
- tracked thread detail

#### Delete Preview

- target existence
- target revision
- direct child count
- descendant count
- whether any tracked thread has an `upstream_thread_id`
- preview summary string for confirmation UI

### Projection Contract

Normal list and detail queries should exclude tombstoned rows by default. An `include_deleted` flag can exist for debugging and later sync tooling, but the TUI should not use it for routine browsing.

## 8. Daemon API Surface Sketch

The daemon should continue to expose JSON-RPC, but the collaboration surface should become more explicit.

### Snapshot Query

- `state/get`
  - includes workstream, work unit, and tracked-thread hierarchy from projection tables

### List Queries

- `workstream/list`
- `workunit/list`
  - optionally scoped by `workstream_id`
- `tracked_thread/list`
  - scoped by `work_unit_id`
- `hierarchy/get`
  - optimized tree response for the Main surface

### Detail Queries

- `workstream/get`
- `workunit/get`
- `tracked_thread/get`

### Mutation Commands

- `workstream/create`
- `workstream/edit`
- `workstream/delete`
- `workunit/create`
- `workunit/edit`
- `workunit/delete`
- `tracked_thread/create`
- `tracked_thread/edit`
- `tracked_thread/delete`

### Confirmation-Gated Destructive Actions

Add one generic preview endpoint:

- `delete/plan`

Request:

- target kind
- target ID

Response:

- target label
- target revision
- affected work unit count
- affected tracked thread count
- whether any affected tracked thread is bound to an upstream thread
- `confirmation_token`
- `requires_typed_confirmation`
- token expiry

Delete commands then require:

- `expected_revision`
- `confirmation_token`

This keeps delete policy enforced by the daemon instead of relying only on UI etiquette.

### Event Notifications

The existing event subscription model remains valid. Add lifecycle notifications for:

- workstream created, edited, deleted
- work unit created, edited, deleted
- tracked thread created, edited, deleted

The TUI should still be able to do snapshot-first boot plus event follow.

## 9. TUI Footer And Composer Mode Model

The current bottom pane should evolve from a steer-only composer into a generic mutation footer for the Main program surface. Review should stay as-is for this pass.

### Conceptual Mode Enum

```rust
enum FooterMode {
    Idle,
    CreateWorkstream,
    EditWorkstream { workstream_id: Id, expected_revision: u64 },
    CreateWorkUnit { workstream_id: Id },
    EditWorkUnit { work_unit_id: Id, expected_revision: u64 },
    CreateTrackedThread { work_unit_id: Id },
    EditTrackedThread { tracked_thread_id: Id, expected_revision: u64 },
    ConfirmDelete {
        target: DeleteTarget,
        expected_revision: u64,
        confirmation_token: String,
        requires_typed_confirmation: bool,
    },
}
```

### Form Behavior

- selecting a workstream enables create child work unit, edit, and delete
- selecting a work unit enables create child tracked thread, edit, and delete
- selecting a tracked thread enables edit and delete
- composer submit sends a daemon command
- on success, composer closes and the TUI waits for snapshot refresh or lifecycle event
- on validation error, composer stays open and shows inline error text

### Suggested Field Sets

#### Create Or Edit Workstream

- title
- objective
- status
- priority

#### Create Or Edit Work Unit

- title
- task statement
- status

#### Create Or Edit Tracked Thread

- title
- notes
- backend kind
- upstream thread ID optional
- preferred cwd optional
- preferred model optional

## 10. Delete Semantics And Confirmation Behavior

### Delete Model

Use tombstones, not hard delete, for all three entity types in MVP.

Reasons:

- preserves local audit history
- makes replay deterministic
- keeps room for later sync and replication
- avoids lying about upstream runtime deletion

### Cascade Behavior

#### Deleting A Workstream

- allowed only as a confirmation-gated cascade
- tombstones the selected workstream
- tombstones all non-deleted child work units
- tombstones all non-deleted descendant tracked threads
- writes explicit delete events for every affected entity in one transaction

#### Deleting A Work Unit

- allowed only as a confirmation-gated cascade
- tombstones the selected work unit
- tombstones all non-deleted child tracked threads
- writes explicit delete events for every affected entity in one transaction

#### Deleting A Tracked Thread

- tombstones only the local tracked-thread record
- does not delete the upstream Codex thread
- if an `upstream_thread_id` exists, the delete preview should say that Orcas tracking will be removed locally and the upstream thread may still exist

### Confirmation UX

Recommended flow:

1. Operator invokes delete on current selection.
2. TUI calls `delete/plan`.
3. Footer switches to `ConfirmDelete`.
4. Footer shows impact summary.
5. For leaf deletes, `Enter` confirms.
6. For cascading deletes, require typed `delete`.
7. TUI sends delete command with `confirmation_token`.

This gives the TUI a clean operator flow while keeping policy enforcement in the daemon.

## 11. Boot, Replay, And Recovery Model

### Boot

On daemon boot:

1. open SQLite database at the Orcas data path
2. run migrations
3. ensure `origin_node_id` exists
4. validate projections against `projection_checkpoint`
5. rebuild projections from the event log if needed
6. only then serve IPC requests

### Replay

Projections should be rebuildable from `event_log` alone. That is the real benefit of keeping the event model explicit.

For MVP, the normal write path can append events and update projections in the same transaction. Replay is then primarily for:

- startup consistency checks
- recovery after interrupted migrations
- store repair tooling later

### Recovery

Because writes are transactional, a crash should leave the database at the last committed state. On restart:

- no partially committed command should appear
- the daemon can emit a warning if projection rebuild was needed
- the TUI reconnects snapshot-first as it already does

### Migration From Current JSON State

The repo currently persists state to `state.json`. The MVP implementation should add a one-time import path:

- if `state.db` does not exist and `state.json` does
- import the current JSON snapshot into SQLite
- generate creation events for imported active rows
- preserve the JSON file as a backup during rollout

That keeps the design grounded in the current repo rather than assuming a greenfield store.

## 12. Explicit Future-Sync Seams Reserved For Later

The design should reserve, but not implement, later sync.

### Reserved Seams

- globally unique entity IDs
- `origin_node_id`
- `command_id` and `event_id`
- per-entity `revision`
- tombstones retained in local history
- canonical event log with ordered `seq`
- upstream reference fields on tracked threads
- `command_receipts` for idempotency

### What Is Deferred

- cloud authority
- authentication and accounts
- remote conflict resolution policy
- bidirectional replication transport
- cross-device merge UX

The intended later evolution is straightforward:

- local daemon stays the write boundary for local operator actions
- the same commands and events can later be replicated
- cloud-authoritative mode can treat the local store as a replica without redesigning the local core

## 13. Recommended Implementation Sequence

### Pass 1: Domain And Store Skeleton

- add `docs/adr` and design docs first
- add domain IDs, command types, and event types in `orcas-core`
- add store traits for command append, event append, projection update, and query
- add `state_db_file` path alongside the current JSON state path

### Pass 2: SQLite Store In `orcasd`

- introduce a minimal SQLite dependency, preferably `rusqlite`
- add migrations
- create `event_log`, `command_receipts`, and projection tables
- add one-time import from existing `state.json`

### Pass 3: Command Handlers And Projector

- implement create, edit, delete command handlers for workstreams, work units, and tracked threads
- apply events into projection tables
- emit daemon lifecycle notifications after commit

### Pass 4: IPC Query And Mutation Surface

- add explicit `tracked_thread/*` methods
- add edit and delete methods for all three entities
- add `delete/plan`
- update `state/get` and hierarchy queries to read projections

### Pass 5: TUI Main Footer/Composer

- replace the current one-off footer logic with a generic Main-surface composer state
- support create, edit, and delete-confirm flows
- keep Review untouched except for consuming the updated snapshot shape if needed

### Pass 6: Repair And Recovery Hardening

- add projection rebuild command or internal repair path
- add startup validation
- add migration tests from JSON snapshot to SQLite

### Test Strategy

Each implementation pass should ship tests at the level it changes.

#### Store And Domain

- unit tests for command validation
- unit tests for delete cascade planning
- event serialization round-trip tests
- projection replay tests from an event sequence

#### SQLite

- temp-directory integration tests against a real SQLite file
- migration tests
- crash-safe idempotency tests around `command_receipts`

#### Daemon IPC

- request/response tests for create, edit, delete, and `delete/plan`
- snapshot-plus-event tests proving the TUI can rebuild from daemon output

#### TUI

- reducer/state-machine tests for footer mode transitions
- composer validation tests
- confirmation flow tests for leaf and cascading deletes

The most important invariant tests are:

- command accepted once even if retried with the same `command_id`
- projection rebuild matches current tables
- tracked-thread delete never claims upstream hard delete
- parent delete cascades are explicit and deterministic
