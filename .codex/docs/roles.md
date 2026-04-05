# Codex lane roles for Orcas

This package is the shared runtime role pack for the Orcas-managed app-server home.

Orcas keeps the checked-in template under `.codex/` and refreshes the runtime `.codex` subtree into the shared app-server `CODEX_HOME`.

This package is set up for two distinct use cases:

1. **Codex-native custom agents / subagents**
   - Files: `.codex/agents/*.toml`
   - Use when a parent Codex session will explicitly spawn role-specific agents.

2. **Orcas-launched standalone Codex sessions**
   - Files: `.codex/orcas/role-instructions/*.md`
   - Use when Orcas starts a Codex thread directly and can set per-thread developer instructions.

The same role content is used in both places so behavior stays aligned.

---

## Recommended split of responsibility

### Put in `developer_instructions`
Use lane identity and hard boundaries here.

Good examples:
- the lane's job
- what it should optimize for
- what it must not touch
- what exact ack text it should return on a fresh thread

Keep this compact and stable.

### Put in `AGENTS.md`
Use repo-wide rules that all lanes should inherit.

Good examples:
- worktree and branch naming rules
- where the frontier ledger lives
- how to record proof status
- required validation commands
- file ownership and directory conventions
- definitions of terms like frontier, gate, promotion-worthy, deferred, closed

Do **not** duplicate the full role text into `AGENTS.md` unless every Codex session in the repo should inherit it.

### Put in a skill
Use a skill only when you have a longer reusable workflow that would otherwise bloat the role.

Good examples:
- frontier-ledger update workflow
- bounded live-smoke checklist
- todo normalization workflow
- promotion report workflow

---

## Why this package uses `developer_instructions`, not `model_instructions_file`

`developer_instructions` is the right place for additive lane behavior.

`model_instructions_file` is a replacement-style mechanism. It is heavier and easier to misuse if your goal is only to add a lane role while still keeping normal built-in behavior and `AGENTS.md` guidance.

The older `instructions` key should be avoided.

---

## Files

```text
.codex/config.toml
.codex/agents/integration.toml
.codex/agents/harness.toml
.codex/agents/feature.toml
.codex/agents/direct.toml
.codex/orcas/role-instructions/integration.md
.codex/orcas/role-instructions/harness.md
.codex/orcas/role-instructions/feature.md
.codex/orcas/role-instructions/direct.md
.codex/skills/direct/SKILL.md
.codex/skills/chat/SKILL.md
.codex/skills/learn/SKILL.md
.codex/skills/propose/SKILL.md
.codex/skills/process/SKILL.md
.codex/skills/i3/SKILL.md
.codex/skills/codex/SKILL.md
.codex/skills/doctor/SKILL.md
.codex/skills/git/SKILL.md
.codex/skills/test/SKILL.md
.codex/skills/agent/SKILL.md
.codex/skills/human/SKILL.md
.codex/skills/clean/SKILL.md
.codex/skills/services/SKILL.md
.codex/docs/roles.md
```

---

## How to use with Orcas direct-launch mode

When Orcas launches a standalone Codex session for one lane, inject the matching role text as the thread's developer instructions.

Minimal pattern:

```json
{
  "settings": {
    "developer_instructions": "<contents of .codex/orcas/role-instructions/integration.md>"
  }
}
```

Then send a tiny first user turn, for example:

```text
ack
```

Expected response:

```text
understood, please proceed with input
```

This keeps the role out of the user prompt and leaves the first real user message free for task input.

---

## How to use with Codex custom agents / subagents

Keep `.codex/config.toml` and `.codex/agents/*.toml` in the repo.

Then, from a parent Codex session, explicitly spawn the role you want. The role files are already narrow enough that each agent has a distinct job:

- `integration`: ledger, frontier truth, promotion gating
- `harness`: runners, wrappers, bounded proof execution, operator docs
- `feature`: proof-only runtime investigation
- `direct`: main operator playbook, backlog intake, and capability dispatch

The `direct` skill and lane are meant to work from a shared, tracked backlog
file in `docs/WORKSTREAM_TODO.md` so user notes can be inserted, reviewed,
and expanded into a planning phase over multiple exchanges.

If you later decide a lane should inherit extra tools or a different model, add those settings to the relevant `.toml` file rather than widening the prompt.

---

## Suggested `AGENTS.md` content for this repo

This package intentionally does **not** overwrite your repo's `AGENTS.md`, because that file is usually already carrying real project conventions.

Recommended shared content for `AGENTS.md`:

- the canonical status / frontier document paths
- the canonical todo / backlog file paths
- worktree naming conventions
- branch naming conventions
- how to record blocked, paused, closed, and deferred states
- what counts as promotion-worthy evidence
- which commands are safe for harness lanes to run
- how to handle scratch instrumentation
- how to summarize findings before handoff

Think of `AGENTS.md` as the repo constitution, and the lane files as narrow job descriptions.

---

## Optional future extension

If you later want even tighter separation for standalone Orcas sessions, you can create one config layer per lane with a top-level `developer_instructions` key and launch Codex against that layer. This package does not generate that variant because Orcas can already inject per-thread developer instructions directly, which is simpler.
