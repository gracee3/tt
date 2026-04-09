# TT / Codex Runtime Contract

## Purpose

This document defines the runtime boundary between TT and Codex.

TT is an orchestration layer around Codex. TT should not fork Codex's runtime
or protocol surface. Instead, TT should discover and use a compatible Codex
installation, validate that it meets TT's requirements, and fail clearly when
it does not.

The current preferred installation model is per-user local installs.

## Ownership

Codex owns:
- agent execution
- thread and turn lifecycle
- app-server websocket transport
- sandbox behavior
- `.codex` user and project state

TT owns:
- project orchestration
- `.tt` user and project state
- worktree and branch policy
- managed-project topology
- director and worker coordination
- TT-specific inspection, testing, and release validation

TT must not require a TT-specific fork of Codex. TT only requires a compatible
Codex build.

## Preferred Install Layout

Per-user install locations are the current target.

Codex binaries:
- `~/.local/bin/codex`
- `~/.local/bin/codex-app-server`

TT binaries:
- `~/.local/bin/tt`
- `~/.local/bin/tt-daemon`
- `~/.local/bin/tt-tui`

Project state:
- Codex user state: `~/.codex/`
- Codex project state: `<repo>/.codex/`
- TT user state: `~/.tt/`
- TT project state: `<repo>/.tt/`

Auth file:
- Codex auth: `~/.codex/auth.json`

TT should keep its own config and state separate from Codex config and state.
For repo-local development checkouts, TT may also load `<repo>/.tt/settings.env`
as a lightweight env overlay for TT/Codex path defaults. Shell env still wins.

## Binary Discovery

TT should resolve Codex binaries in this order:

1. explicit env overrides
2. `PATH`
3. optional TT-managed install root

Recommended env vars:
- `TT_CODEX_BIN`
- `TT_CODEX_APP_SERVER_BIN`

Current TT enforcement is stricter than the longer-term discovery model:
- if `TT_CODEX_BIN` / `TT_CODEX_APP_SERVER_BIN` are set, TT uses them
- otherwise TT requires:
  - `~/.local/bin/codex`
  - `~/.local/bin/codex-app-server`
- TT now fails fast when that contract is not met

Current app-server listen URL overrides used by TT:
- `CODEX_APP_SERVER_LISTEN_URL`
- `TT_APP_SERVER_LISTEN_URL`

Current Codex auth requirement enforced by TT:
- `~/.codex/auth.json` must exist and be readable
- live e2e app-server launches use `CODEX_HOME=$HOME/.codex`
- TT fails fast if the auth file is missing

TT should continue to support explicit listen URL override for testing and
runtime control.

## Required Codex Runtime Capabilities

TT currently depends on Codex supporting all of the following:

1. `codex-app-server` binary
- websocket listen URL can be explicitly configured
- app-server starts locally without requiring TT-specific patches

2. Thread lifecycle
- start thread
- resume thread
- read thread
- list threads

3. Turn lifecycle
- create a turn
- observe turn status until completion or failure
- read completed turn data

4. Turn history visibility
- completed turns can be recovered with enough history for TT to extract worker
  handoffs
- item history or equivalent final agent output must be recoverable after turn
  completion

5. Agent/runtime overrides
- model selection
- reasoning effort
- sandbox mode
- approval policy
- per-thread cwd / workspace targeting

## Strongly Recommended Codex Capabilities

These are not all hard requirements today, but they materially improve TT
reliability and release management:

1. Machine-readable version output
- example: `codex-app-server --version`

2. Machine-readable capability discovery
- example: `codex-app-server --capabilities --json`

3. Health endpoints
- readiness endpoint
- health endpoint

4. Stable, documented behavior for:
- thread history loading
- turn item persistence
- spawn semantics
- app-server reconnect expectations

## Compatibility Contract

TT should validate compatibility against Codex, not assume it.

The practical contract is:
- TT releases track Codex stable releases
- TT mainline continuously validates against Codex alpha releases

Recommended lanes:

1. Codex stable lane
- required for TT release
- TT release notes should pin the validated Codex stable version or range

2. Codex alpha lane
- continuous compatibility signal
- catches protocol and runtime drift before the next stable release

TT should add an explicit compatibility check surface, preferably through:
- `tt codex app-servers`
- an internal runtime probe used by `tt open` and the live harness

That output should include:
- resolved `codex` binary path
- resolved `codex-app-server` binary path
- detected versions
- configured listen URL
- compatibility status
- project `.codex` root
- project `.tt` root

Current repo-scoped runtime inspection:
- `tt codex app-servers` reports TT's repo-local daemon socket path plus the
  effective configured Codex app-server listen URL for the current repo
- it performs repo-scoped metadata and reachability inspection only
- it does not enumerate host-wide processes or listening ports

## CI Expectations

TT CI should be able to run against prebuilt Codex artifacts without compiling
Codex from source.

Recommended TT CI matrix:

1. TT-only
- unit tests
- store / daemon / CLI / TUI tests
- no Codex runtime dependency

2. TT + Codex stable
- required gate for TT release
- uses prebuilt Codex stable binaries

3. TT + Codex alpha
- continuous integration lane
- validates upcoming compatibility

4. TT live managed-project scenarios
- topology scenario on each PR if feasible
- heavier multi-round scenarios on a dedicated or nightly lane

Current live scenario examples:
- `managed-project-git-worktree`
- `managed-project-rust-taskflow-four-round`
- `managed-project-rust-taskflow-integration-pressure`

## Artifact Expectations For the Codex Fork

To support TT cleanly, the Codex fork should ideally provide:

1. Installable binaries
- `codex`
- `codex-app-server`

2. Stable alpha and stable release channels
- prebuilt artifacts that TT CI can consume directly

3. Clear compatibility metadata
- version
- release channel
- optional capability inventory

4. Reliable local startup
- explicit listen URL support
- stable readiness behavior

5. Stable thread/turn/history behavior
- enough for TT to extract structured worker handoffs from live runs

## What TT Should Not Require

TT should not require:
- a TT-specific Codex fork
- compiling Codex from source for normal end users
- merged TT binaries into Codex binary names
- shared `.tt` and `.codex` state roots

Building Codex from source is acceptable for TT development and local live e2e,
but it should not be the default product assumption.

## Recommended Next TT Work

1. Add explicit Codex discovery env vars to TT docs and internal probe output
2. Add compatibility reporting in the internal Codex runtime probe
3. Teach TT CI to consume prebuilt Codex stable and alpha artifacts
4. Keep live managed-project scenarios as the runtime compatibility gate

The current TT repo CI shape should include:
- a TT-only lane with Codex shim binaries
- a Codex stable lane using prebuilt artifacts
- a Codex alpha lane using prebuilt artifacts

## Handoff Summary

If another agent is maintaining the Codex fork, the ideal ask from TT is:

- publish per-user installable `codex` and `codex-app-server` binaries
- keep alpha and stable channels distinct
- expose version and, ideally, capability metadata
- preserve explicit app-server listen URL support
- keep thread, turn, and history behavior stable enough for TT-managed worker
  handoff extraction
- make those artifacts easy for TT CI to consume directly
