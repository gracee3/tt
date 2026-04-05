# Codex role pack for Orcas

Included:
- project-scoped Codex custom-agent files under `.codex/agents/`
- matching plain role-instruction files for Orcas direct injection
- a direct skill under `.codex/skills/`
- companion capability skills under `.codex/skills/`
- `docs/roles.md` with usage notes and developer-instructions guidance

Recommended startup pattern for the primary lanes:
1. set the lane's `developer_instructions`
2. send `ack`
3. expect `understood, please proceed with input`
4. send the real task

Direct-oriented operator flows should use the `direct` lane and its matching
skill when the task is about coordinating other lanes, maintaining operator
context, or deciding the next best action.
