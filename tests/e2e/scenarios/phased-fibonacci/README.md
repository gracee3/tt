# Phased Fibonacci Scenario

This scenario simulates the operator workflow across multiple phases:

- the supervisor starts with a vague idea and a phase plan
- the operator approves each bounded step
- the harness exports supervisor, operator, and agent prompts to disk
- the same worktree lane is updated across all phases
- the final result is a real C Fibonacci project on disk

The checked-in [`plan.md`](./plan.md) is the source of truth for phase gates and acceptance criteria.
