# Managed Project Rust Taskflow Four Round

This scenario creates a brand-new git repo with `tt project init`, activates the
director/dev/test/integration topology, and runs a seeded four-round managed
project scenario against real Codex threads.

It verifies:

- TT can create a new repo and bootstrap a managed project without a preexisting checkout.
- `tt project director --scenario rust-taskflow-four-round` drives a multi-round project run.
- TT records scenario state, round progression, and landing approval in managed-project state.
- TT records liveness policy and watchdog progress so slow but healthy turns do not look hung.
- TT writes a JSONL progress stream so the director and worker subagents can be monitored while they run.
- The final repo still builds and tests cleanly as a Rust crate.
