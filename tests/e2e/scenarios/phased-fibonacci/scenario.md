# Phased Fibonacci

This scenario simulates a longer operator workflow:

1. The supervisor starts with a vague implementation idea.
2. The operator approves the plan.
3. The harness turns that plan into code in phases and seeds the matching report state.
4. After each phase, the supervisor creates a proposal from the current report.
5. The operator approves the next path.
6. The same worktree lane is used from start to finish.

The harness also exports each phase's prompts to disk so the workflow can be inspected after the run.

## Source of Truth

- [`plan.md`](./plan.md) defines the phase gates, scope, and acceptance criteria.
- The harness exports the prompts and reports to disk for inspection.
- The local `state.json` in the scenario-local XDG data directory under `target/e2e/xdg/` is updated between phases so the proposal/report loop stays coherent.
- The final result is expected to be a buildable Fibonacci C project in a real git worktree.

## Outputs

- `target/e2e/artifacts/<run-id>/phased-fibonacci/plan.md`
- `target/e2e/artifacts/<run-id>/phased-fibonacci/phases/*/agent-prompt.txt`
- `target/e2e/artifacts/<run-id>/phased-fibonacci/phases/*/supervisor-prompt.txt`
- `target/e2e/artifacts/<run-id>/phased-fibonacci/phases/*/operator-prompt.txt`
- `target/e2e/artifacts/<run-id>/phased-fibonacci/phases/*/proposal.md`
- `target/e2e/reports/<run-id>/phased-fibonacci/phases/*/*.txt`
- `target/e2e/worktrees/<run-id>/phased-fibonacci/lane`

## Cleanup

Use `make clean-e2e` from the repository root to remove generated XDG state, logs, artifacts, reports, and worktrees.
