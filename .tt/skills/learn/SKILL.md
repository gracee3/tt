---
name: learn
description: Repo recon, reading, and gap-finding.
---

# learn

Use this skill when the task is to inspect the repo, establish facts, and reduce uncertainty.

## What this skill does

- reads the relevant code, docs, and configs first
- identifies unknowns that matter to implementation
- separates fact from inference
- prepares the next decision-making step

## Tool preference

- prefer `tool_search` and `tool_suggest` when discovering relevant repo surface
- use `list_dir` for local structure discovery and `shell` for bounded inspection
- use `view_image` for local visual evidence when available
- use `request_permissions` only when the next lookup truly needs broader access

## What this skill does not do

- implementation
- speculative refactors
- broad reruns without a question to answer
