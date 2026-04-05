# Validate Workflows Todo

Working notes for the `validate-workflows` stream. I will keep this organized as notes come in and regroup items when needed.

## Inbox

- Investigate daemon socket connection failures when `orcasd` is already running but the CLI cannot connect cleanly.
- Move role/planning design into a separate workstream so this stream stays focused on workflow validation.
- Move supervisor/plan mode design into a separate workstream because it is a large, independent effort.

## Now

- Reproduce the remaining daemon socket failure mode where `orcasd` exists but the CLI still cannot complete the spawn path cleanly.

## Next

- Split role/planning design into a separate workstream.
- Split supervisor/plan mode design into a separate workstream.

## Later

- 

## Open Questions

- What exact daemon/socket state produces the current `orcasd`-running-but-unreachable failure?

## Completed

- Added `orcas roles list` and `orcas roles info <role>`.
- `orcas roles list` now uses a merged role view with local overrides marked by `*`.
- Added `orcas worktrees` to iterate workstreams and print workstream id/name, status, repo root, branch name, and worktree path.
- Updated `orcas codex spawn` to validate role and daemon availability before creating worktrees or branches.
- Ordinary CLI commands now use one-shot daemon checks instead of hidden retries; daemon start/restart paths still own retry behavior.
- Generated Orcas workstream branches now default to `worktree/<slug>`, while worktree directories stay under the configured worktree root, defaulting to `~/worktrees/orcas/<slug>`.
- `orcas workstream delete` now accepts a positional workstream selector and resolves exact ids or exact names.
- `orcas daemon status` no longer keeps stale shared runtime rows when the explicit lane path has already been removed from disk.
