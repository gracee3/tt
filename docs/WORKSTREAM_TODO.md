# Direct Workstream Todo

Tracked backlog for the active direct skill thread.

## Inbox

- Clarify how `todo`, `chat`, `learn`, `test`, `handoff`, and `docs` should map to skills, developer instructions, or a separate collaboration mode in TT.
- Define `todo` as the ledger skill that forks a TT thread/history into a new todo branch, writes pasted notes into a single canonical branch-local `TODO.md`, and auto-merges the result back while preserving useful context.
- Model `todo` as a single implementation with explicit modes for note intake, review/questioning, plan emission, and chat-style dialogue.
- Expose `todo` subcommands for the modes so the state machine is explicit, such as `todo note`, `todo review`, `todo plan`, and `todo chat`.
- Make `todo`, `review`, and `plan` available at every TT layer where it makes sense, including lane, repo, and worktree scopes.
- Track the persistent note/review/plan artifacts in git alongside the snapshot/workspace state when that produces the cleanest durable history.
- Use lane-level artifacts only for lightweight coordination summaries, and keep canonical `TODO.md`, `PLAN.md`, and `REVIEW.md` in repo/worktree scope.
- Use the worktree as the live editable copy and merge those durable docs back into the repo on close/commit.
- Keep thread snapshots and worktree metadata in TT-managed metadata under `~/.tt`, while repo git holds code plus durable docs like `TODO.md`, `PLAN.md`, and `REVIEW.md`.
- Confirm that chat history and worktree state should stay out of the repo git history except for deliberate durable exports or summaries.
- Prefer the simplest control-plane storage that preserves turn/worktree boundaries; treat worktree commits after each turn as the important durable boundary.
- Define what belongs in the turn commit versus what stays only in TT metadata after a turn completes.
- Commit canonical `TODO.md`, `PLAN.md`, and `REVIEW.md` artifacts to the repo by default so they remain part of the durable project history.
- Decide whether the branch handler should remain TT-native or become a Codex collaboration mode, with TT acting as the macro/config layer that selects worktree, permissions, environment, and cleanup behavior.
- Define the TT collaboration/branch-handler mode shape: inputs, outputs, lifecycle, commit behavior, and cleanup behavior.
- Prefer separate mode-specific handlers over one monolith, but keep a single shared TT engine for branch/worktree creation, auto-merge, resolve, prune, and cleanup so TT can reuse it internally.
- Define the shared TT engine responsibilities: branch creation, permission model, workspace binding, merge/prune/cleanup, metadata resolution, and a cleanup/prune review mode.
- Add a cleanup/prune review mode with a `git status`-like interface for selecting tracked files to add/merge before close, plus a second confirmation for untracked or unstaged dirty files before close/prune/delete.
- Expose that cleanup/prune review mode as a user-facing `diff` command/mode.
- Prefer separate handlers for `todo`, `chat`, `learn`, `handoff`, and `diff`, backed by a single shared branch/worktree engine underneath.
- Keep `diff` shared across top-level roles unless a strong reason emerges to specialize it.
- Treat `diff` as a shared tool/mode used heavily by `integrate`, not something owned exclusively by `integrate`.
- Give the shared branch/worktree engine a compact API around `fork`, `attach`, `set_policy`, `commit_or_merge`, `prune`, `cleanup`, `status`, and `restore`.
- Define the four top-level role handlers as `todo`, `test`, `integrate`, and `develop`, each with access to the shared `chat`, `learn`, `handoff`, and `diff` modes.
- Decide the semantics of the top-level `close`, `split`, and `park` commands for those four roles.
- Define the four top-level roles as `todo`, `test`, `integrate`, and `develop`.
- Define `develop` as the active implementation role for feature work, code changes, and branch-local execution.
- Define `test` as the validation role for CI/build/test work, harness authoring, and branch-local verification.
- Define `integrate` as the repo-level branch manager that watches commits, auto-merges, resolves conflicts, writes branching changelogs, and keeps the branching strategy structured.
- Make the branch lifecycle operations (`split`, `close`, `park`) available to all four top-level roles, with behavior driven by the policy of the command/subcommand that initiated the action.
- Define `close` as the localized cleanup/teardown of the active worktree/branch, including policy-driven merge of ready commits and pruning afterward.
- Define `split` as the creation of a new child workstream/thread with one of the four top-level roles applied to it.
- Default `split` to always create a new worktree and new thread instance using the current branch/policy unless the operator specifies otherwise.
- Default `split` to the same mode plus a child branch/worktree, but allow the user to override mode and branch explicitly.
- Make the default `split` behavior the same for all top-level roles unless a role-specific override is explicitly defined later.
- `split` should leave the existing thread active, start the new thread in a new window if available or headless otherwise, and return the new resume thread-id.
- Allow `split` to target an explicit role/branch such as `split test main`, or to inherit the current role/branch shape by default.
- Define `park` as a close-like suspension that keeps the same resume-id and preserves the worktree exactly as-is for later reattachment.
- Make the default `close` and `park` behavior the same for all top-level roles unless a role-specific override is explicitly defined later.
- Commit tracked files at the end of each turn by default, while reserving `close`/`merge`/`clean` for operator or supervisor commands.
- Let workspace policy decide whether new top-level threads may be spawned automatically and whether a branch/worktree can auto-merge and clean itself into its parent.
- Allow `develop` to spawn a new top-level thread automatically when needed.
- Allow `test` and `integrate` to spawn new top-level threads automatically when their policies allow it.
- Keep `todo` manual for spawn/close behavior unless explicitly overridden by policy.
- Allow `test` to commit tracked files automatically at the end of a turn as part of writing tests, harnesses, and changelog-style results.
- Let `integrate` follow developer branches in parallel, auto-merge and resolve as needed, and scope its immediate task from any incoming context.
- Define the `integrate` lifecycle around watching commit events, deciding when to auto-merge versus wait for the operator, and producing a branching changelog on close.
- Allow `integrate` to auto-merge based on explicit policy and on confidence from diff/test signals.
- Default `todo` to a fresh branch/worktree and use the existing `TODO.md` or create an empty one if needed.
- Allow `TODO.md` to be sectioned so only one subsection is actively worked on at a time, with the active section optionally chosen automatically by policy or branch metadata.
- Define `chat` as the user-facing discuss-only mode that can be toggled on from any state, does not persist Codex thread history, and has no implementation side effects.
- Define `handoff` as a skill that packages the current state, ownership, repo/focus context, and next steps for transfer to another role, agent, or snapshot boundary.
- Define `test` as a basic developer-instruction layer for validation, CI/build/test work, and harness/test authoring.
- Define `plan` as a `todo` submode that converts clarified backlog items into bounded, inspectable action plans.
- Define `learn` as a skill/subcommand system for on-demand recon and context building that gathers environment and repository context and may optionally write a canonical generated context file.
- Define `docs` as an internal developer-instruction/helper layer that turns settled behavior into durable repo docs and CLI/reference output.
- Treat `roles` only as TT-internal mapping metadata for the new skills; do not assume any preexisting active roles or skills outside the set being defined now.
- Use a tracked project-level backlog file on disk as the shared source of truth between exchanges.
- Preserve pasted notes, bugs, and text with enough detail to recreate the issue or context later.
- Support a commit-frequent workflow where the direct skill updates the backlog and returns a diff/changelog to the user.
- Keep clarification questions small and related, and let the user continue steering the backlog as needed.
- Allow an `insert` mode for normal note intake.
- Allow a `review` mode that prompts for missing requirements, undefined edges, and open gaps.
- Allow a `plan` mode that uses repo recon and source inspection to produce an implementation plan.
- Let the user view and revise the plan as work evolves.
- Explore the tt/codex directory hierarchy and sandboxing architecture for lane-local runtimes, including host-global `~/.codex` and `~/.tt`, lane-scoped worktrees/repos, and per-lane runtime/config roots.
- Treat lane-local config and runtime state as mostly rendered artifacts generated by `tt`, while allowing operator-managed drift to remain contained at the ephemeral worktree/lane layer.
- Require each lane to be keyed by an operator-provided human-readable field that `tt` normalizes into a slug for the lane directory name, while preserving the original label as metadata.
- Initialize lane repos by fresh clone, and prefer an `org/repo`-style repo key/path layout under the lane root when practical.
- Treat the lane-level `shared/home` as a read-only overlay source, with overrides pushed into more-local worktree roots so changes do not affect other worktrees.
- Make worktree runtimes persist until explicit operator cleanup rather than garbage-collecting them on inactivity.
- Define the explicit cleanup ladder for a worktree runtime, including whether cleanup removes only the runtime, the worktree, the repo clone, or all associated workspace bindings.
- Prefer single-repo workspaces within a lane, while allowing multiple repos to coexist under the same lane for coordination across groups of agents.
- Use a lane-root path shape that nests repo identity under `repos/<org>/<repo>` and workspace/runtime state under `workspaces/<workspace-slug>/` with explicit `runtime/`, `worktree/`, and `home/` children.

## Now

- Walk through the `todo`, `chat`, `learn`, `test`, `handoff`, and `docs` concepts one by one and clarify whether each is a skill, developer instruction, or both.
- Pin down the `todo` boundaries: branch-for-write, note ingestion, single canonical `TODO.md`, auto-merge, and optional review prompting.
- Pin down the `diff` boundaries: primarily current worktree/branch plus parent worktree/branch, with `git status`-like staging and cleanup confirmation behavior.
- Pin down the `todo` mode transitions: note -> review -> plan -> close, plus explicit read-only chat branches when the operator wants dialogue without disk writes.
- Pin down the exact `todo` subcommand semantics and the operator-visible outputs for each mode.
- Default `todo` to a fresh branch/worktree fork when toggled on, unless the operator explicitly resumes an existing todo branch.
- Auto-merge and cleanup/prune the todo branch when it is toggled off or explicitly closed.
- Pin down the `chat` boundaries: toggle-from-anywhere, no persisted thread history, discuss-only behavior, and no implementation side effects.
- Make all modes enterable from any state and ensure they return to the main thread cleanly after merge/close.
- Pin down the `handoff` boundaries: what state is preserved, what is summarized, what is excluded, what operator-supplied context is required, and what the receiving role must know to resume safely.
- Pin down the `test` boundaries: accepted conditions, expected outputs, branch/rebase behavior, harness/test authoring scope, and how it distinguishes expected failures from regressions.
- Pin down the `plan` boundaries: how it transitions from backlog/review into a formal plan, and what it should hand back to execution.
- Pin down the `learn` boundaries: prepared recon statements, context discovery, optional generated output, and how it informs later modes without becoming execution context.
- Pin down the `docs` boundaries: what it generates, when it runs, and how it keeps reference output aligned with settled behavior.
- Decide how the TT-internal skill mapping layer should represent the new skills without depending on any legacy active role set.
- Preserve the operator-provided lane label alongside the slug so naming remains ergonomic and deterministic.
- Allow repo labels to follow GitHub-style `org/repo` naming or equivalent operator-friendly labels, with slug normalization handled by `tt`.
- Keep shared lane config read-only for overlay consumers and require local worktree-level state for per-thread or per-worktree deviations.
- Keep lane-local runtimes reusable across detach/reattach cycles until the operator explicitly retires them.

## Next

- Normalize future pasted notes into this file.
- Ask only the smallest useful follow-up questions when the backlog has gaps.

## Later

- Expand the backlog into a planning phase once requirements are sufficiently complete.

## Resolved

- TT-managed metadata under `~/.tt` stores snapshots, worktrees, and workspace bindings; repo git stores code plus durable docs like `TODO.md`, `PLAN.md`, and `REVIEW.md`.
- Lane-level coordination stays lightweight; canonical durable docs live in repo/worktree scope.
- The branch handler stays TT-native and behaves as a TT collaboration mode macro/config layer over Codex.
- TT keeps a small internal mapping/routing layer for skills and handlers; the user-facing concepts are the modes/roles themselves.
- The collaboration/branch-handler surface uses separate mode-specific handlers over one shared branch/worktree engine.
- The shared engine API is `fork`, `attach`, `set_policy`, `commit_or_merge`, `prune`, `cleanup`, `status`, and `restore`.
- The shared engine may create new worktrees automatically by default, with explicit operator override when needed.
- Workspace policy controls whether top-level threads may auto-spawn and whether a worktree may auto-merge/clean itself into its parent.
- `set_policy` remains mode-specific by default.
- Cleanup and prune flows require a final `are you sure?` confirmation if dirty files or unmerged files would be lost.
- `park` means suspend and retain the worktree/resume-id without cleanup or prune.
- The cleanup/prune review mode is user-facing as `diff`, with `git status`-like staging and confirmation behavior.
- `diff` stays shared across top-level roles and is used heavily by `integrate`.
- The default engine tracking policy is read-only unless the mode handler overrides it; `rw` means auto-track/auto-merge for engine-chosen files, not filesystem write permission.
- Filesystem write access remains available independently, and the operator can toggle `ro`/`rw` at any time.
- `close` prunes the active worktree/branch after policy-driven merge of ready commits.
- `split` always creates a new child thread/worktree by default, leaves the parent active, and returns a new resume thread-id.
- `todo` defaults to a fresh branch/worktree and reuses or creates `TODO.md` as needed.
- `TODO.md` may be sectioned, and policy may choose the active subsection automatically.
- `close`, `split`, and `park` share the same defaults across the four top-level roles unless a role-specific override is defined later.
- `todo`, `test`, `integrate`, and `develop` are the four top-level roles.
- `todo` owns the canonical `TODO.md` / `PLAN.md` / `REVIEW.md` flow.
- `plan` is a `todo` submode that attaches to the currently referenced `TODO.md` section and shares its title/linkage.
- `chat` is the user-facing discuss-only mode with no persisted Codex thread history and no implementation side effects.
- `learn` is a skill/subcommand system for on-demand recon/context building.
- `handoff` is a skill.
- `test` is the validation role and may commit tracked files automatically at the end of a turn.
- `develop` is the implementation role and may auto-spawn a new top-level thread.
- `integrate` is the repo-level branch manager and may auto-merge based on policy or confidence from diff/test signals.
- `split` defaults to the same mode plus a child branch/worktree, but explicit mode and branch overrides are allowed.
- `split` always creates a new worktree/thread instance and may be started headless until i3 wiring exists.
- `close` and `park` defaults are the same across top-level roles unless explicitly overridden later.
- `test` and `integrate` may auto-spawn new top-level threads when policy allows it; `todo` stays manual unless overridden by policy.
- `test` commits tracked files automatically as part of writing tests, harnesses, and changelog-style results.
- `integrate` follows developer branches in parallel, auto-merges and resolves as needed, and scopes immediate work from incoming context.
- `integrate` watches commit events, decides when to auto-merge or wait for the operator, and produces a branching changelog on close.
- `integrate` can auto-merge from explicit policy or from confidence derived from diff/test signals.
- `lane`/`repo`/`worktree` artifact split is: lane = lightweight coordination summaries; repo/worktree = canonical durable docs plus active editable copies.
- The canonical backlog file for this workflow is `docs/WORKSTREAM_TODO.md`; any additional workstream-specific todo files should be explicit and scoped.
- Backlog updates should be committed at stable boundaries, typically when a section is materially changed or a mode/workflow is being switched.
- Lane runtimes should use multiple worktrees per lane, with one lane-local control plane/runtime namespace.
- Lane-local generated config should be rendered by `tt` by default, with any operator-managed drift confined to ephemeral worktree roots.
- Lane names should keep both the operator-provided human-readable label and the normalized slug.
- Repo keys under a lane should prefer `org/repo`.
- Shared lane overlay state should remain read-only; overrides stay localized to the active worktree/runtime root.
- Detached worktree runtimes should survive until explicit cleanup.
- The default cleanup action should retire the runtime/worktree after merge; deeper cleanup that removes the repo clone and workspace metadata should be explicit.
- Workspaces should stay single-repo by default; cross-repo coordination should happen at the lane level.
- The canonical files for lane/repo/worktree state are `lane.toml`, `repo.toml`, `workspace.toml`, `runtime/`, and `shared/`, with lane metadata, repo metadata, workspace binding, runtime state, and rendered templates/defaults split accordingly.

## Remaining Questions

- None at the moment.

## Completed

- Identified the need for a dedicated direct skill.
- Established the project-level tracked backlog file for the direct workflow.
- Wrote the TT v2 architecture doc and added the parallel v2 crate scaffold for `tt-domain`, `tt-store`, `tt-codex`, `tt-git`, `tt-daemon`, `tt-ui-core`, `tt-tui`, and `tt-cli`.
- Implemented a sqlite-backed `.tt` overlay store with project, work unit, binding, and merge-run persistence plus round-trip tests.
- Implemented a Codex home/session-index adapter that discovers `~/.codex`, derives Codex state paths, and loads a lightweight thread catalog.
- Implemented a TT daemon status/dashboard service that summarizes overlay counts and optional Codex catalog state.
- Implemented a git/worktree inspection layer that discovers current checkouts, lists worktrees, and derives merge-readiness summaries for the daemon.
- Added a runnable `tt-tui` dashboard entrypoint that renders Codex, repo, and overlay state from the new v2 services.
- Added a local daemon request/response API and thin `tt-cli` client commands for status, repo, and entity CRUD.
- Added a Unix-socket daemon transport under `.tt/runtime/ttd.sock` and switched the CLI/TUI to route through it when available.
- Added Codex thread lifecycle wiring through the daemon, CLI, and TUI, including thread start/resume/read/list flows with `.codex`-aware resolution.
- Added workspace/merge reconciliation helpers that inspect git state, refresh workspace bindings, and upsert merge-run records from the same source of truth.
- Added workspace actions and daemon-state-first lifecycle operations (`prepare`, `merge-prep`, `authorize-merge`, `execute-landing`, `prune`, `close`, `park`, `split`) plus a simplified TUI command guide and matching thin CLI surface.
- Isolated the raw store-shaped CRUD/status CLI surface behind a `legacy` namespace so the main interface can stay centered on lifecycle and reconcile flows.
- Implemented the lane filesystem layout scaffold and the `tt lane init|inspect|cleanup` CLI surface.
- Added explicit lane/workspace manifest fields and surfaced discovered lane roots in `tt doctor`.
- Added lane attachment mirroring so `tt lane attach|detach` updates tracked-thread binding state and records the attachment list in the workspace manifest.
- Added `tt lane list` plus a cleanup guard that refuses to delete live-attached workspaces without `--force`.
- Added the new role/lifecycle CLI surface for `todo`, `develop`, `test`, `integrate`, `chat`, `learn`, `handoff`, `diff`, `split`, `close`, and `park`, backed by role prompts and shared branch/worktree helpers.
