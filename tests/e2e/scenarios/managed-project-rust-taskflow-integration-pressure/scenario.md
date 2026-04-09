# Managed Project Rust Taskflow Integration Pressure

This scenario creates a brand-new git repo with `tt project init`, activates the
director/dev/test/integration topology, and runs a seeded four-round managed
project scenario that introduces a deterministic integration blocker in round 3.

It verifies:

- TT can keep the same multi-round project moving when integration is blocked.
- The seeded worker handoff contract captures blockers and next steps explicitly.
- TT records watchdog progress and liveness policy so long waits remain diagnosable.
- TT writes a JSONL progress stream so the director and worker subagents can be monitored while they run.
- The final round resolves the blocker and reaches a merge-ready completed state.
- The final repo still builds and tests cleanly as a Rust crate.
