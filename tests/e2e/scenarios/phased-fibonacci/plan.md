# Phased Fibonacci Plan

## Goal

Build a small C Fibonacci CLI across five gated phases, keeping the supervisor plan visible and the code buildable at each step.

## Phases

### Phase 0: Scope

- Read the plan and summarize risks, assumptions, and the implementation order.
- Do not edit files yet.
- Acceptance: the supervisor has a concise scoping report and a plan proposal can be approved.

### Phase 1: Skeleton

- Create the project skeleton in the declared worktree.
- Add `main.c`, a `Makefile`, and a smoke-test target.
- Acceptance: the project builds and prints a basic Fibonacci sequence.

### Phase 2: CLI and Validation

- Add command-line arguments for sequence length and formatting.
- Validate bad input cleanly.
- Acceptance: invalid inputs fail with a useful error and the smoke test still passes.

### Phase 3: Library Split

- Move the Fibonacci logic into reusable `fib.c` and `fib.h`.
- Keep the CLI thin and preserve behavior.
- Acceptance: the build stays green and the sequence output is unchanged.

### Phase 4: Tests and Polish

- Add a repeatable test script and tighten the final build/test path.
- Clean up warnings and obvious rough edges.
- Acceptance: `make test` passes and the worktree contains the finished code.

## Operating Rules

- Stay on the current phase only.
- Keep the worktree buildable before asking for the next approval.
- Export every assignment prompt and phase report to disk.
- Use the operator approvals to keep the supervisor plan on track.
