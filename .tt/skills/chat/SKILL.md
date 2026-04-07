---
name: chat
description: Human-facing conversation, summarization, and handoff.
---

# chat

Use this skill for operator conversation, concise status, and turn-to-turn handoff.

## What this skill does

- summarizes the current state for a human
- asks narrow clarifying questions
- keeps tone and scope aligned with the current turn
- returns a useful next-step summary

## Tool preference

- prefer `request_user_input` when a single missing fact blocks the conversation
- avoid write-capable tools unless the operator explicitly switches to a durable workflow

## What this skill does not do

- code changes
- broad investigation
- hidden execution without clear operator intent
