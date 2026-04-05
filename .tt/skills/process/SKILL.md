---
name: process
description: Process lifecycle management and long-lived process state.
---

# process

Use this skill for starting, inspecting, coordinating, and retiring processes that must stay managed over time.

## What this skill does

- tracks process lifecycle state
- coordinates start, stop, restart, and health checks
- keeps process ownership explicit
- records the minimum operational detail needed to recover state

## Runtime surface

Prefer the typed `tt skill process ...` entrypoint:

- `tt skill process status --pid <pid>`
- `tt skill process status --name <pattern>`
- `tt skill process inspect --pid <pid>`
- `tt skill process inspect --name <pattern>`
- `tt skill process start [--cwd <path>] [--name <label>] <command...>`
- `tt skill process stop --pid <pid>`
- `tt skill process stop --name <pattern>`
- `tt skill process restart [--cwd <path>] [--name <label>] <command...>`
- `tt skill process signal --pid <pid> --signal <TERM|HUP|INT|...>`
- `tt skill process signal --name <pattern> --signal <TERM|HUP|INT|...>`
- `tt skill process tree --pid <pid>`
- `tt skill process tree --name <pattern>`

## What this skill does not do

- change process semantics without a clear reason
- replace specialized service or TT coordination
- improvise operational policy
