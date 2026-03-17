# Codex Thread Supervision v1

This slice adds Orcas-side mirror and monitoring support for Codex app-server threads.
It now also includes Orcas-native assignment metadata for binding a Codex thread to Orcas workflow objects, plus Orcas-native next-turn, steer, and interrupt supervisor decisions with explicit human review.

## Canonical Boundaries

Codex app-server remains canonical for:

- thread existence
- loaded/runtime status
- turn lifecycle
- item/event delivery
- actual turn execution

Orcas remains canonical for:

- persisted mirror state used by Orcas clients
- monitor attachment intent and attachment status shown to operators
- thread-to-workflow assignment metadata
- supervisor proposal state for Codex threads
- human approval workflow for Codex next-turn sends
- human approval workflow for Codex active-turn steer sends
- human approval workflow for Codex active-turn interrupt sends
- stale-basis validation before Orcas sends into a Codex thread

This slice does not store Orcas workflow metadata inside Codex thread history.

## Supported v1 Behavior

- discover existing Codex threads, including externally created and headless threads
- read persisted thread history from app-server and persist the normalized Orcas mirror
- best-effort attach a thread for future live monitoring using documented `thread/resume` behavior
- detach a thread in Orcas so the operator surface reports `history only`
- assign a Codex thread to an Orcas workstream, work unit, and supervisor
- pause, resume, and release Orcas-native Codex-thread assignments
- auto-generate a single human-reviewable next-turn proposal for an active assigned idle thread
- approve and send that proposal through documented `turn/start`
- let an operator propose a steer message for an active assigned thread
- let an operator author or revise steer text in the TUI before approval/send
- let an operator author or revise multiline steer text in the TUI before approval/send
- approve and send that steer through documented `turn/steer`
- let an operator propose an interrupt for an active assigned thread
- approve and send that interrupt through documented `turn/interrupt`
- reject a proposal without sending
- mark a proposal stale when the assignment or Codex thread basis changes before send
- persist thread mirror state and turn state across Orcas restart
- persist Codex-thread assignment state across Orcas restart
- persist Codex-thread supervisor-decision state across Orcas restart
- show in the TUI:
  - loaded status
  - live attach status
  - assignment badge / assignment panel
  - pending human approval / pending steer approval / pending interrupt approval / stale / sent / rejected decision state
  - latest next-turn, steer, or interrupt proposal rationale
  - authored steer text for pending review
  - edit pending steer before send
  - recent decision history for the selected thread, including superseded steer revisions and replacement links
  - persisted turn history
  - aggregated item text
  - turn lifecycle snapshots
  - source kind when app-server exposes it

## Monitor Semantics

Thread monitor state is explicit:

- `detached`: Orcas has history or discovery data only
- `attaching`: Orcas is attempting best-effort live attachment
- `attached`: Orcas requested live monitoring and is ingesting future app-server events
- `errored`: Orcas could not establish best-effort live monitoring

`detached` does not mean the thread is dead. It only means Orcas is not claiming an active live-monitor attachment for that thread.

## Persistence Semantics

Orcas persists:

- normalized thread summaries
- persisted turn/item history already known to Orcas
- turn lifecycle snapshots used by operator views

Orcas does not attempt to reconstruct transient deltas it never observed.

## Assignment Semantics

Codex-thread assignment is an Orcas-native object.

- It binds `codex_thread_id` to `workstream_id`, `work_unit_id`, and `supervisor_id`.
- It is persisted in Orcas collaboration state, not in Codex thread history.
- A thread can still be monitored whether assigned or unassigned.
- Creating an assignment never sends a turn.
- Creating an assignment for an active thread never interrupts, steers, or queues a send.

The daemon enforces:

- at most one active assignment per `codex_thread_id`
- paused assignments are not active
- released assignments are not active
- released assignments remain queryable for audit/history

Current assignment lifecycle operations:

- `create`
- `get`
- `list`
- `pause`
- `resume`
- `release`

## Supervisor Decision Semantics

Supervisor next-turn decisions are Orcas-native objects.

- They are bound to an active `CodexThreadAssignment`.
- They are persisted in Orcas collaboration state, not in Codex thread history.
- Orcas generates them only when the assigned thread is idle.
- Orcas does not silently send them. Human approval is required in this slice.
- Orcas keeps at most one open pending decision per assignment.

Current decision lifecycle operations:

- `list`
- `get`
- `approve_and_send`
- `reject`

Decision status meanings:

- `proposed_to_human`: pending human approval
- `approved`: reserved internal transition during the send path
- `sent`: Orcas successfully called documented `turn/start`
- `rejected`: human rejected the proposal
- `stale`: basis changed before Orcas could send
- `superseded`: defined in the model, but not yet a primary path in this slice

Interrupt decisions reuse the same Orcas-native object with:

- `kind = interrupt_active_turn`
- `proposal_kind = operator_interrupt`
- `basis_turn_id` required
- no proposed text payload
- documented send path limited to `turn/interrupt`

Interrupt proposals are operator-initiated only in this slice. Orcas does not auto-generate them just because a thread is active.

Steer decisions reuse the same Orcas-native object with:

- `kind = steer_active_turn`
- `proposal_kind = operator_steer`
- `basis_turn_id` required
- `proposed_text` required
- documented send path limited to `turn/steer`

Steer proposals are operator-initiated only in this slice. Orcas does not auto-generate them just because a thread is active.
Steer text is operator-authored in the TUI in this slice; Orcas does not synthesize steer text automatically.
The current TUI compose flow supports bounded multiline editing with cursor movement, newline insertion, save, and cancel.

## Basis / Stale Validation

For Codex-thread next-turn proposals, Orcas uses a conservative basis:

- proposals are generated only when the thread is idle
- the basis is the latest known `last_seen_turn_id` at generation time
- `approve_and_send` re-checks that:
  - the assignment is still active
  - the thread is still idle
  - the latest known basis still matches the decision basis
  - the decision is still pending human review

If any of those checks fail, Orcas does not send. The decision is marked stale and remains Orcas-native audit state.

For interrupt proposals, Orcas uses a conservative active-turn basis:

- proposals are created only when the assignment is active and the thread currently has an active turn
- the basis is the active `active_turn_id` at proposal time
- `approve_and_send` re-checks that:
  - the assignment is still active
  - the decision is still pending human review
  - the thread still has an active turn
  - the current `active_turn_id` still matches `basis_turn_id`

If any of those checks fail, Orcas does not interrupt. The decision is marked stale and the upstream Codex thread is left unchanged.

For steer proposals, Orcas uses the same conservative active-turn basis:

- proposals are created only when the assignment is active and the thread currently has an active turn
- the basis is the active `active_turn_id` at proposal time
- `approve_and_send` re-checks that:
  - the assignment is still active
  - the decision is still pending human review
  - the thread still has an active turn
  - the current `active_turn_id` still matches `basis_turn_id`
  - `proposed_text` is still non-empty

If any of those checks fail, Orcas does not steer. The decision is marked stale and the upstream Codex thread is left unchanged.

## Steer Proposal Semantics

- Steer proposals are only available for assigned active threads with a current active turn.
- Orcas keeps the one-open-decision-per-assignment invariant.
- If an open next-turn decision already exists, Orcas rejects steer proposal creation as conflicting in this slice.
- If an open interrupt decision already exists, Orcas rejects steer proposal creation as conflicting in this slice.
- If an assignment pauses or releases while a steer proposal is pending, the proposal becomes stale and cannot be sent.
- If the active turn changes or completes naturally before approval, the steer proposal becomes stale.
- Pending steer edits use immutable replacement: Orcas supersedes the previous pending steer decision and creates a new pending steer decision with the revised text.
- Sent, rejected, stale, and superseded steer decisions remain immutable.
- The TUI keeps recent per-thread decision history visible so superseded steer revisions remain inspectable with their `superseded_by` linkage.
- A successful steer send does not start a new turn. Orcas waits for normal Codex event ingestion to reflect the continued in-flight turn state.
- Orcas uses documented `turn/steer` constraints only:
  - `expectedTurnId` is required and must match the active turn
  - steer fails if there is no active turn
  - steer does not start a new turn
  - steer does not accept turn-level overrides such as model, cwd, sandbox policy, or output schema

Current operator text-entry support is TUI-only in this slice. Orcas does not yet expose authored steer text entry through a separate CLI command surface.

## Interrupt Proposal Semantics

- Interrupt proposals are only available for assigned active threads with a current active turn.
- Orcas keeps the one-open-decision-per-assignment invariant.
- If an open next-turn decision already exists, Orcas rejects interrupt proposal creation as conflicting in this slice.
- If an assignment pauses or releases while an interrupt proposal is pending, the proposal becomes stale and cannot be sent.
- If the active turn completes naturally before approval, the interrupt proposal becomes stale.
- A successful interrupt send does not invent a new turn id. Orcas waits for normal Codex event ingestion to reflect the interrupted terminal state.

## Bootstrap Proposal Semantics

Assignments persist a `bootstrap_state`.

- new active assignments begin with bootstrap pending
- when the assigned thread is idle and no open decision exists, Orcas proposes a bootstrap next turn first
- when that bootstrap proposal is generated, assignment bootstrap state becomes `proposed`
- if bootstrap is approved and sent, bootstrap state becomes `sent`
- if bootstrap is rejected, bootstrap state becomes `not_needed`
- if bootstrap becomes stale before send, bootstrap state returns to `pending`

Bootstrap text is deterministic and template-based in this slice. Orcas does not yet rely on a separate autonomous reasoning subsystem for Codex-thread next-turn proposals.

## Non-goals In This Slice

- PTY attach or PTY replay
- exact replay of transient deltas Orcas missed before attach
- whole-thread kill semantics
- process-tree kill semantics
- automatic supervisor writing into Codex threads
- mutation of app-server’s persisted thread log format
- autonomous sending without human approval

## IPC Surface Added

- `threads/list_loaded`
- `thread/read_history`
- `thread/attach`
- `thread/detach`

Existing `thread/start`, `thread/read`, `thread/get`, `thread/resume`, `turn/start`, `turn/steer`, and `turn/interrupt` remain in place.

Assignment IPC added:

- `codex_assignment/create`
- `codex_assignment/get`
- `codex_assignment/list`
- `codex_assignment/pause`
- `codex_assignment/resume`
- `codex_assignment/release`

Supervisor decision IPC added:

- `supervisor_decision/list`
- `supervisor_decision/get`
- `supervisor_decision/propose_steer`
- `supervisor_decision/replace_pending_steer`
- `supervisor_decision/propose_interrupt`
- `supervisor_decision/approve_and_send`
- `supervisor_decision/reject`
