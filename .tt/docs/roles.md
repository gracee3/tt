# TT lane roles for TT

This package is the shared runtime role pack for the TT-managed app-server home.

TT keeps the checked-in template under `.tt/` and refreshes the runtime `.tt` subtree into the shared app-server `RUNTIME_HOME`.

This package is set up for two distinct use cases:

1. **TT-native custom agents / subagents**
   - Files: `.tt/agents/*.toml`
   - Use when a parent TT session will explicitly spawn role-specific agents.

2. **TT-launched standalone TT sessions**
   - Files: `.tt/tt/role-instructions/*.md`
   - Use when TT starts a TT thread directly and can set per-thread developer instructions.

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

Do **not** duplicate the full role text into `AGENTS.md` unless every TT session in the repo should inherit it.

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
.tt/config.toml
.tt/agents/integration.toml
.tt/agents/harness.toml
.tt/agents/feature.toml
.tt/agents/direct.toml
.tt/tt/role-instructions/integration.md
.tt/tt/role-instructions/harness.md
.tt/tt/role-instructions/feature.md
.tt/tt/role-instructions/direct.md
.tt/skills/direct/SKILL.md
.tt/skills/chat/SKILL.md
.tt/skills/learn/SKILL.md
.tt/skills/propose/SKILL.md
.tt/skills/process/SKILL.md
.tt/skills/i3/SKILL.md
.tt/skills/tt/SKILL.md
.tt/skills/doctor/SKILL.md
.tt/skills/git/SKILL.md
.tt/skills/test/SKILL.md
.tt/skills/agent/SKILL.md
.tt/skills/human/SKILL.md
.tt/skills/clean/SKILL.md
.tt/skills/services/SKILL.md
.tt/docs/roles.md
```

---

## How to use with TT direct-launch mode

When TT launches a standalone TT session for one lane, inject the matching role text as the thread's developer instructions.

Minimal pattern:

```json
{
  "settings": {
    "developer_instructions": "<contents of .tt/tt/role-instructions/integration.md>"
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

## How to use with TT custom agents / subagents

Keep `.tt/config.toml` and `.tt/agents/*.toml` in the repo.

Then, from a parent TT session, explicitly spawn the role you want. The role files are already narrow enough that each agent has a distinct job:

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

If you later want even tighter separation for standalone TT sessions, you can create one config layer per lane with a top-level `developer_instructions` key and launch TT against that layer. This package does not generate that variant because TT can already inject per-thread developer instructions directly, which is simpler.
