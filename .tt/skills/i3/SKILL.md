---
name: i3
description: i3 window-manager coordination and workspace control.
---

# i3

Use this skill when the task involves i3, sway, or desktop/window-manager coordination for the operator.

## Scope

- owns desktop arrangement and window focus intent
- coordinates operator-visible surfaces for TT and related tools
- treats window management as a controlled operator action, not a side effect
- does not change application semantics

## Discovery

- detect whether the session is X11 or Wayland
- locate the active window manager and available control command
- inspect environment variables that identify display, session, and runtime context
- identify which UI surface should be primary for the current turn

## Tools

- `i3-msg` when i3 is the active window manager
- `swaymsg` when the session is Wayland/sway-backed
- shell helpers for session inspection and desktop state queries
- `ttui` or other repo-provided operator surfaces when the turn needs a visible control plane
- daemon/session inspection where the desktop state depends on live runtime state

## Runtime surface

Prefer the typed `tt skill i3 ...` entrypoint:

- `tt skill i3 status`
- `tt skill i3 attach`

- `tt skill i3 focus [--workspace <name>]`

- `tt skill i3 workspace focus --workspace <name>`
- `tt skill i3 workspace move --workspace <name>`
- `tt skill i3 workspace list`

- `tt skill i3 window focus --criteria <i3 criteria>`
- `tt skill i3 window move --criteria <i3 criteria> --workspace <name>`
- `tt skill i3 window close --criteria <i3 criteria>`
- `tt skill i3 window info --criteria <i3 criteria>`

- `tt skill i3 message <raw args...>`

## Runtime State

- current display/session type
- active workspace and focused window
- which terminal or UI should stay visible
- whether the session is local, remote, or nested
- what window changes are already in flight

## Protocol

- receive a desktop intent from `direct`
- perform the minimum window action needed to satisfy it
- prefer explicit focus/move/show/hide actions over broad layout rewrites
- confirm the resulting desktop state in plain language

## Finish

- report the resulting workspace, focus, and visible surfaces
- note whether the layout is stable for the next turn
- leave the operator in a sensible place to continue

## Failure Modes

- if the session type cannot be determined, report that uncertainty instead of guessing
- if the window manager command is unavailable, say so and stop
- if the requested arrangement is ambiguous, ask for the smallest clarifying detail
- if the desktop state conflicts with the requested action, do not force it blindly
