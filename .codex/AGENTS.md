# AGENTS.md

This repository uses a lane-based agent workflow.

Role identity belongs in lane-specific `developer_instructions` or `.codex/agents/*.toml` files.
This file is for **shared repo rules only**.
Keep it short, accurate, and practical.

## Operating model

- Treat this file as the repo constitution.
- Treat lane roles as narrow job descriptions layered on top.
- Prefer the smallest truthful step that advances the current gate.
- Preserve findings in-tree so the next turn can pick up without replaying the whole investigation.

## Source-of-truth documents

Keep these paths accurate for the repo. Edit this section if the canonical paths change.

- `docs/FRONTIER_PROOF_STATUS.md`
  - Canonical frontier ledger.
  - Record what is proven, what is blocked, what is paused, and the next gate.
- `docs/WORKSTREAM_TODO.md`
  - Canonical tracked todo/backlog file for the active workstream.
  - Use this as the shared project-level inbox, review queue, and planning seed.
- `docs/WORKSTREAM_INTEGRATION_TODO.md`
  - Integration backlog and ledger follow-up items.
- `docs/WORKSTREAM_HARNESS_TODO.md`
  - Harness backlog and bounded live-smoke follow-up items.
- `docs/WORKSTREAM_RUNTIME_TODO.md`
  - Runtime / feature investigation backlog.
- `docs/NEXT_MILESTONES.md`
  - Cross-lane milestone summary.

If this repo uses different filenames, update this file immediately rather than letting stale paths persist.

## Shared rules for every lane

1. **Respect lane boundaries.**
   - Do not silently absorb work that belongs to another lane.
   - If the next correct step belongs elsewhere, say so and hand off cleanly.

2. **Prefer bounded work.**
   - Choose the narrowest change or run that can answer the next question.
   - Avoid broad refactors, broad retries, and speculative cleanup during an active frontier.

3. **Separate fact from hypothesis.**
   - Record what was directly observed.
   - Label inference as inference.
   - Label proposed next steps as proposed next steps.

4. **Preserve evidence in-tree.**
   - Update the relevant ledger, backlog, or operator doc in the same turn as the finding.
   - Do not leave key state only in chat text.

5. **Keep closed frontiers closed.**
   - Do not reopen a closed or deferred frontier without direct new evidence.
   - "Maybe" or "worth another try" is not enough.

6. **Do not over-claim.**
   - Liveness is not semantic proof.
   - A partial trace is not root-cause closure.
   - An exploratory runtime patch is not promotion-worthy by default.

7. **State the next gate explicitly.**
   - Every status update should end with the next decision-grade step.

## Definitions

Use these terms consistently.

- **Proven**: supported by direct evidence strong enough for the current decision.
- **Blocked**: cannot currently proceed because a required prerequisite is missing or failing.
- **Paused**: intentionally not active right now, but not closed.
- **Deferred**: intentionally moved out of the current frontier.
- **Closed**: decision made; do not revisit without direct new evidence.
- **Frontier**: the current narrow problem boundary under active investigation.
- **Gate**: the next evidence threshold required before promotion, escalation, or closure.
- **Promotion-worthy**: clean enough to merge or promote without depending on unresolved speculative behavior.
- **Liveness-only**: proves the path runs, but not that the semantics are trustworthy.
- **Semantically trustworthy**: supported by direct comparison, contract checks, or other evidence appropriate to the lane.

## Evidence standard

When you claim a result, record enough detail that another operator can verify it without guessing.

Include, when relevant:

- exact commit SHA or worktree/branch context
- exact files changed
- exact commands run
- exact input or boundary used
- artifact paths produced
- whether the run was baseline, candidate, or control
- whether baseline and candidate were isolated
- whether the result is liveness-only or semantically trustworthy
- what remains unproven

Do not compress away the distinction between "no artifact emitted", "artifact emitted but mismatched", and "run did not complete".

## Worktrees, scratch changes, and risky edits

- Use isolated worktrees or branches for risky runtime investigation, scratch instrumentation, and one-off probes.
- Keep branch names short and descriptive. A good default is `<lane>/<topic>`.
- Call out scratch instrumentation explicitly in summaries.
- Do not present scratch-only runtime edits as merge-ready unless they were cleaned up and validated.
- If a scratch probe produced a real finding, preserve the finding in docs even if the probe itself should not land.

## Documentation discipline

When a frontier moves, update the docs that define the repo state.

Minimum expectation:

- ledger reflects the new proven/blocked/paused state
- relevant backlog reflects the new next actions
- stale next-step text is removed or rewritten
- ambiguity about whether a path is closed, paused, or still active is eliminated

## Handoff format

Use this structure for lane handoffs and status reports whenever practical:

- **Lane:**
- **Goal:**
- **Status:** completed / partial / blocked
- **What changed:**
- **Files changed:**
- **Commands run:**
- **Artifacts:**
- **Ledger/backlog updated:** yes / no, with paths
- **What is now proven:**
- **What remains unproven:**
- **Next gate:**
- **Open risks or questions:**

Keep handoffs crisp. Favor exactness over narrative.

## Done criteria

Do not call work done until all of the following are true:

- the narrow task objective was met or explicitly blocked
- the relevant docs were updated to reflect reality
- the exact validation or evidence basis was recorded
- the next gate is clear
- no stale statement remains that would mislead the next operator

## What belongs in developer_instructions instead

Do **not** duplicate lane identity here.

Put these in lane-specific `developer_instructions` or `.codex/agents/*.toml`:

- lane job and scope
- lane-specific do-not rules
- lane-specific ack behavior
- model/reasoning/sandbox choices for that lane

Keep this file stable and shared; keep lane files narrow and replaceable.
