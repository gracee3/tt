# Orcas Collaboration v1 Scope And Non-Goals

This document defines the narrow intended v1.

## In Scope For v1

- one human supervising work through Orcas
- one Orcas supervisor coordination loop
- multiple workers
- one or a small number of active workstreams
- explicit work units with explicit dependency edges
- one active assignment per worker
- one active assignment per work unit
- Codex-backed worker execution through the existing Orcas runtime
- structured worker reports
- explicit supervisor decisions
- durable visibility into assignment, report, and session state
- honest interruption and resumability semantics grounded in daemon evidence

## What Makes v1 Genuinely Supervisor-Centered

The minimum bar for v1 is not "multiple threads exist." The minimum bar is:

- work is explicitly decomposed into work units
- assignments are explicit
- reports are explicit
- decisions are explicit
- the supervisor controls all cross-worker coordination

Without those properties, Orcas is still mostly acting like a prompt router.

## Non-Goals

- no direct worker-to-worker coordination in v1
- no autonomous spawning of top-level work without supervisor decision
- no hidden claims of continuity when runtime evidence is missing
- no fully autonomous orchestration without human oversight
- no attempt to solve every future collaboration pattern up front
- no general multi-human consensus or permissions model
- no marketplace of heterogeneous remote agents
- no speculative distributed scheduler
- no peer mesh or gossip protocol between workers
- no requirement that raw Codex transcripts be the user-facing workflow record

## Explicit v1 Exclusions

### No Direct Worker Messaging

Workers may influence each other only through:

- reports
- supervisor synthesis
- new assignments

There is no worker-to-worker chat channel in v1.

### No Silent Continuation

If Orcas cannot prove continuity after daemon replacement or connection loss, v1 must not present the assignment as continuously running. It may show cached state, terminal state, or lost state, but not invented continuity.

### No Autonomous Top-Level Decomposition

Workers may recommend splitting or creating follow-up work, but only the supervisor can create that workstream-level structure in v1.

### No Free-Running Workers

Workers should not run indefinitely. Every assignment needs:

- a bounded objective
- stop conditions
- a report expectation

### No Giant Ontology

V1 should keep the object model small enough to implement and inspect.

The protocol does not need:

- subtask taxonomies for every domain
- complex confidence calculus
- ten layers of dependency semantics

## Human Visibility And Control Expectations

The human should be able to inspect:

- workstreams
- work units
- dependency status
- active assignments
- reports
- supervisor decisions
- worker session continuity state

The human should be able to control:

- priority
- interruption
- redirect
- completion
- abandonment
- blocker resolution

The human does not need to author every worker prompt by hand.

## Honest Runtime Semantics

The following are acceptable claims in v1:

- "worker session is attached to thread X"
- "assignment turn is still attachable"
- "only cached terminal state is available"
- "continuity was lost"

The following are not acceptable claims unless proven:

- "the worker kept running uninterrupted"
- "this assignment resumed seamlessly"
- "the previous runtime context definitely survived"

## Narrowest Viable v1

The narrowest viable v1 implementation is:

- one workstream
- two or more work units
- explicit dependency handling
- two workers
- one assignment loop per worker
- structured reports
- supervisor decisions that either continue, redirect, or complete work

That is enough to demonstrate real supervisor-mediated collaboration without overbuilding.

## Deferred Areas

These can wait until after v1:

- worker specialization and capability routing beyond simple tags
- automatic work balancing across many workers
- richer artifact indexing and search
- schema validation strictness beyond a practical first pass
- direct worker handoff patterns
- collaborative editing semantics between simultaneous workers
- full approval UX integration
- browser-specific control surfaces
