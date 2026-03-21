# Scenario: Proposals and Decisions

## Goal

Verify that a proposal can be created for a seeded work unit, reviewed, and converted into a recorded supervisor decision without waiting on a live worker turn.

## Steps

1. Seed the minimal workstream, work unit, assignment, report, worker-session, and open proposal state required for the deterministic flow.
2. Inspect the seeded proposal details.
3. Approve the proposal and record a decision.
4. Verify the proposal and decision are linked.

## Expected Result

- The CLI can show the proposal and the resulting decision without manual database inspection.
- The scenario does not depend on `assignment/start` or a live worker turn.
- The scenario does not invoke the model-backed proposal generator.
