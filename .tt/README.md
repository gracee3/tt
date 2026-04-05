# TT role pack for TT

Included:
- project-scoped TT custom-agent files under `.tt/agents/`
- matching plain role-instruction files for TT direct injection
- a direct skill under `.tt/skills/`
- companion capability skills under `.tt/skills/`
- `docs/roles.md` with usage notes and developer-instructions guidance

Recommended startup pattern for the primary lanes:
1. set the lane's `developer_instructions`
2. send `ack`
3. expect `understood, please proceed with input`
4. send the real task

Direct-oriented operator flows should use the `direct` lane and its matching
skill when the task is about coordinating other lanes, maintaining operator
context, or deciding the next best action.
