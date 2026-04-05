---
name: services
description: Background service lifecycle coordination.
---

# services

Use this skill when the task is to manage long-lived background services.

## What this skill does

- tracks service start, stop, and health intent
- coordinates lifecycle changes across dependent processes
- keeps service ownership and state explicit

## Runtime surface

Prefer the typed `tt skill services ...` entrypoint:

- `tt skill services status daemon`
- `tt skill services status app-server`
- `tt skill services inspect daemon`
- `tt skill services inspect app-server`
- `tt skill services start daemon`
- `tt skill services start app-server`
- `tt skill services stop daemon`
- `tt skill services stop app-server`
- `tt skill services restart daemon`
- `tt skill services restart app-server`
- `tt skill services reload daemon`
- `tt skill services reload app-server`

## What this skill does not do

- application feature work
- ad hoc service tweaks without a recorded reason
- broad platform changes unless requested
