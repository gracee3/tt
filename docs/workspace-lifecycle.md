## Tracked-Thread Workspace Lifecycle

Orcas manages tracked-thread workspaces through an explicit, supervisor-driven lifecycle. Orcas owns intent, the worker performs git mutations, and the daemon only observes local git state read-only.

### State layers

#### Supervisor intent
Canonical on `tracked_thread.workspace`.

Defines the lane, including:
- repository root
- worktree path
- branch name
- base ref / base commit
- landing target
- sync policy
- cleanup policy
- current workspace status

#### Worker-observed state
Reported through structured `workspace_report`.

Used to persist bounded observed fields such as:
- workspace status
- last reported head commit

#### Daemon-observed state
Computed on demand from local git inspection.

Read-only and used for:
- missing worktree detection
- invalid worktree detection
- branch / HEAD visibility
- dirty state
- detached HEAD
- base / landing target comparison warnings where derivable

### Lifecycle flow

#### 1. Workspace declaration
A tracked thread may carry a dedicated workspace contract. This is the canonical supervisor-owned definition of the git lane the worker should use.

#### 2. Worker binding
Worker sessions bind explicitly to a tracked thread through `WorkerSession.tracked_thread_id`, so the correct workspace contract is available during prompt rendering.

#### 3. Prepare workspace
Supervisor may trigger:

```bash
orcas tracked-threads prepare-workspace <thread>
```

This creates a standardized workspace operation instructing the worker to create or normalize the declared lane.

#### 4. Refresh workspace

Supervisor may trigger:

```bash
orcas tracked-threads refresh-workspace <thread>
```

This creates a standardized workspace operation instructing the worker to sync the lane according to the declared policy without landing changes.

#### 5. Merge prep

Supervisor may trigger:

```bash
orcas tracked-threads merge-prep <thread>
```

This produces a bounded readiness assessment using:

* supervisor intent
* latest worker-reported head
* daemon-inspected local state
* latest merge-prep operation/report status

Readiness is surfaced as:

* `ready`
* `not_ready`
* `blocked`
* `unknown`

with explicit reasons.

#### 6. Landing authorization

Supervisor may trigger:

```bash
orcas tracked-threads authorize-merge <thread>
```

Authorization is only allowed when merge prep is ready. The authorization records:

* tracked thread
* authorized head commit
* landing target
* supervisor identity
* timestamp
* linked merge-prep basis

Authorization is explicit and auditable. It does not execute the merge.

#### 7. Landing execution

Supervisor may trigger:

```bash
orcas tracked-threads execute-landing <thread>
```

Execution is only allowed when there is a valid current authorization. The worker receives a dedicated landing contract tied to:

* authorization id
* authorized head commit
* landing target

The worker performs the landing. Orcas captures structured landing results and updates authorization/execution lifecycle state.

#### 8. Prune workspace

Supervisor may trigger:

```bash
orcas tracked-threads prune-workspace <thread>
```

Prune is conservatively gated and intended to close the lane after successful landing or explicit safe retirement. In the current implementation, Orcas only permits prune when there is a successful landing basis or the lane is already explicitly retired. The worker performs cleanup and emits structured prune results. Orcas surfaces intentional closure distinctly from accidental missing worktree state.

### Safety model

Orcas does **not** perform git mutations itself in this workflow.

Division of responsibility:

#### Supervisor

* declares intent
* triggers operations
* reviews readiness
* authorizes landing
* decides when to prune

#### Worker

* performs git/worktree mutations
* reports structured workspace and execution results
* refuses unsafe operations when required by contract
* performs bounded prune cleanup only when explicitly authorized by the prune contract

#### Daemon

* persists operational records
* inspects local git state read-only
* gates risky flows based on explicit rules
* surfaces audit and lifecycle state to CLI/TUI

### Operator-visible workflow summary

Typical happy path:

1. Create or inspect tracked-thread workspace intent.
2. Run `prepare-workspace`.
3. Run `refresh-workspace` as needed.
4. Run `merge-prep`.
5. Run `authorize-merge`.
6. Run `execute-landing`.
7. Run `prune-workspace`.

### Completion status

This feature is complete in substance:

* **v1**: canonical workspace intent, worker contracts, worker reports
* **v2**: daemon-side read-only inspection and warnings
* **v3**: standardized operations, merge prep, authorization, landing execution, prune/closure

Further work, if any, should be treated as expansion rather than missing core lifecycle, for example:

* campaign / integration-branch orchestration
* broader policy automation
* generic workflow extraction
* richer revocation / invalidation engines
