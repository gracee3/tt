# Managed Project Thread Control

This scenario verifies the per-thread control loop for a managed project:

- `tt project control` can mark a worker role `manual_next_turn`
- the director pauses before the next worker turn and records that the thread is now under manual control
- the thread remains attached and inspectable while Codex TUI can watch it live
- switching the same role back to `director` resumes the director-led workflow

The scenario uses the seeded `taskflow` project shape so the control toggle is
tested against the same live managed-project runtime as the multi-round
scenarios. The director keeps the worker thread live while the operator can
watch it in Codex TUI, then the demo pauses the `test` role at the next worker
boundary, records the manual takeover, and finally resumes director control to
finish the seeded run.
