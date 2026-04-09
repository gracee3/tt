# TT Managed Project Contract

Project: `tt`
Repository root: `/home/emmy/openai/tt`
Base branch: `main`

## Coordination Model
- The operator talks to the director.
- The director plans, dispatches, and arbitrates for the project.
- Workers only communicate with the director.
- Peer-to-peer worker coordination is out of scope.

## Roles
- `director`: coordinates the operator, plans the project, dispatches work, and owns handoffs.
- `dev`: implements the assigned code slice only and reports concrete changes.
- `test`: validates the assigned changes and reports exact failures.
- `integration`: prepares landing, merge readiness, and cleanup.

## Phase Vocabulary
- `plan`: turn operator intent into scope and milestones.
- `todo`: capture actionable items and traceability.
- `dispatch`: assign work to a role and a worktree.
- `develop`: implement the assigned slice.
- `test`: validate the change set.
- `integrate`: prepare merge readiness and landing.
- `docs`: update project documentation and handoff notes.
- `merge`: request approval and land the project.

## Handoff Format
- `status`: `blocked`, `needs-review`, or `complete`
- `changed_files`: list of paths
- `tests_run`: list of commands
- `blockers`: list of blockers or `[]`
- `next_step`: the next concrete action

## Escalation Rules
- Workers escalate questions and blockers to the director.
- The director escalates merge/landing approval to the operator when needed.
- Workers do not change branch strategy or project topology on their own.

## Rules
- Stay inside the assigned worktree and scope.
- Do not widen scope without director approval.
- Keep evidence in the handoff, not in prose alone.
