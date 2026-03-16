# Orcas Collaboration v1 Runtime Mapping

## Layering

The collaboration protocol is above the current runtime substrate.

```text
Human
  -> Supervisor
       -> Orcas collaboration objects
            -> Orcas daemon state and IPC
                 -> Codex app-server threads, turns, items, events
```

Codex app-server is the execution substrate.

Orcas collaboration v1 is the coordination and control protocol above it.

That distinction matters because:

- Codex provides execution context and stream events
- Orcas provides workflow objects, decisions, and durable coordination semantics

## Mapping Table

| Orcas concept | Primary home | Codex mapping | Notes |
| --- | --- | --- | --- |
| `Workstream` | Orcas daemon persistent state | none | Pure Orcas control-plane object |
| `Work Unit` | Orcas daemon persistent state | none | Pure Orcas scheduling object |
| `Assignment` | Orcas daemon persistent state | one or more turns in one thread | Assignment history is Orcas-owned even when execution happens in Codex |
| `Worker` | Orcas daemon persistent state | none | Logical actor, not a Codex primitive |
| `Worker Session` | Orcas daemon live state plus persistent metadata | primary Codex thread, optional active turn | Session continuity depends on Orcas runtime evidence |
| `Report` | Orcas daemon persistent state | derived from worker turn output and explicit report contract | Not just the raw transcript |
| `Decision` | Orcas daemon persistent state | may trigger turn start, interrupt, or reassignment | Pure Orcas coordination object |
| `Artifact` | Orcas daemon persistent state | may point to repo files or generated outputs mentioned in items or reports | Stable reference layer |
| `Dependency` | Orcas daemon persistent state | none | Pure Orcas graph edge |

## Mapping To Orcas Daemon State

The daemon already owns:

- live upstream connection state
- thread summaries
- turn lifecycle state
- active turn registry
- recent events
- snapshot and query APIs

Collaboration v1 extends that with new Orcas-owned state:

- workstreams
- work units
- assignments
- workers
- worker sessions
- reports
- decisions
- dependency graph
- artifact registry

The daemon is the natural place to persist and query that state because it already owns local truth for runtime continuity.

## Mapping To Worker Sessions

In v1, a worker session should map to one primary Orcas-managed Codex thread.

Why one thread per worker session:

- it gives each worker a stable local execution context
- it keeps recovery and resumability rules tractable
- it avoids conflating worker identity with transient turns

The session record should carry:

- `thread_id`
- latest known `turn_id`
- daemon instance that proved continuity
- resumability classification
- last-known attachment reason

## Mapping To Codex Threads

Codex threads are the long-lived conversational substrate for a worker session.

Recommended v1 mapping:

- one `Worker Session` -> one primary Codex thread
- one worker may reuse the same thread across multiple assignments
- supervisor redirects or retries may continue in the same thread if the session remains valid

This gives the worker continuity of local context while keeping assignment boundaries explicit at the Orcas layer.

## Mapping To Codex Turns

Codex turns are execution segments inside a worker session.

Recommended v1 mapping:

- one assignment normally begins with one `turn/start`
- an assignment may span multiple turns if the supervisor explicitly issues `continue` on the same assignment
- only one active turn per worker session at a time in v1

That means the important identity is:

- assignment identity at the Orcas layer
- turn identity at the Codex execution layer

They are related, but not the same thing.

## Mapping To Codex Item And Event Streams

Codex item and event streams are the observational substrate.

Orcas should use them to:

- update runtime continuity state
- collect recent output
- detect completion or interruption
- capture artifact references
- support report assembly and review

But Orcas should not treat raw event streams as the collaboration protocol.

V1 reports and decisions should be explicit Orcas objects stored separately from raw event history.

## Runtime Evidence And Honest Claims

The existing daemon already distinguishes:

- active attachable turns
- completed or failed terminal turns
- lost continuity
- unknown query-only turn state

Collaboration v1 should inherit these semantics directly.

### Strongest Evidence

If Orcas can prove:

- the same daemon instance still owns the live turn
- the turn is active
- `turn/attach` succeeds

then the assignment may be presented as live-resumable.

### Medium Evidence

If Orcas can prove:

- the thread still exists
- the worker session still points to that thread
- the active turn is gone or no longer attachable

then the session may be reused, but the assignment is not continuous.

### Weak Evidence

If Orcas only has:

- cached recent output
- terminal turn state
- query-only thread reads

then Orcas may present historical state, but not a live resumption claim.

### No Evidence

If the thread anchor itself is gone or conflicting, Orcas should treat the session as lost.

## Required IPC Shape Changes

The collaboration layer does not require immediate broad runtime changes, but it does imply new Orcas-owned IPC and event surfaces such as:

- `workstreams/*`
- `work_units/*`
- `assignments/*`
- `reports/*`
- `decisions/*`
- `workers/*`

The rule should stay the same as the current daemon model:

- snapshot-first queries for bootstrap
- explicit subscription for live updates
- Orcas-owned summaries rather than raw Codex schema mirroring

## Suggested Event Categories

The daemon event stream will likely need Orcas-owned collaboration events such as:

- `workstream_updated`
- `work_unit_updated`
- `assignment_updated`
- `worker_session_updated`
- `report_submitted`
- `decision_recorded`

Those should sit alongside the existing runtime events:

- turn updated
- item updated
- output delta
- warning

## Example Flow Mapping

```text
Supervisor creates work unit WU-12
  -> Orcas persists WU-12

Supervisor creates assignment A-44 for Worker B
  -> Orcas persists A-44
  -> Worker B session S-B binds to thread T-900

Supervisor starts execution
  -> Orcas calls turn/start on thread T-900
  -> Codex creates turn U-3
  -> Orcas tracks U-3 in active turn registry

Worker finishes and returns structured report text
  -> Codex emits item and turn completion events
  -> Orcas stores raw turn output
  -> Orcas stores Report R-44 as a first-class object

Supervisor records decision D-44
  -> Orcas closes A-44
  -> Orcas updates WU-12 state
```

The collaboration packet should be readable without raw Codex envelopes, even though those envelopes remain part of the runtime substrate.
