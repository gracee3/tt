---
name: test
description: Validation, harness, and evidence capture.
---

# test

Use this skill when the task is to validate behavior, run harnesses, or capture evidence.

## What this skill does

- chooses bounded validation steps
- captures the smallest useful proof
- keeps runs reproducible
- reports what was and was not proven

## Tool preference

- prefer `shell` and `write_stdin` for repeatable validation runs
- use `apply_patch` only when adding or adjusting harness or test files is part of the validation work
- use `update_plan` when the validation path needs a structured checklist
- use `request_permissions` only when the test itself requires broader access

## What this skill does not do

- rewrite product semantics
- broaden scope just to get more coverage
- treat liveness as semantic proof
