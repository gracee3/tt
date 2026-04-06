# TT CLI Reference

This file is the command-line reference for `tt` as implemented in [crates/tt/src/main.rs](/home/emmy/openai/tt/crates/tt/src/main.rs) and [crates/tt/src/remote.rs](/home/emmy/openai/tt/crates/tt/src/remote.rs).

## Global Options

These flags are accepted before any subcommand.

- `--server-url <URL>`: operator server base URL. Env: `TT_SERVER_URL`
- `--operator-api-token <TOKEN>`: bearer token for operator-server APIs. Env: `TT_OPERATOR_API_TOKEN`
- `--tt-bin <PATH>`: override the local TT binary path
- `--listen-url <WS_URL>`: override the upstream TT app-server WebSocket URL
- `--inbox-mirror-server-url <URL>`: enable inbox mirroring to a server URL
- `--cwd <PATH>`: override the default working directory for the command
- `--worktree-root <PATH>`: override the default worktree root for workstream and TT spawn commands
- `--model <MODEL>`: override the default model for the command
- `--connect-only`: require attach-only mode for the current process
- `--force-spawn`: legacy runtime override for spawn-capable processes
- `-h, --help`
- `-V, --version`

## Top Level

- `tt daemon ...`
- `tt doctor`
- `tt remote ...`
- `tt events ...`
- `tt workstream ...`
- `tt workunit ...`
- `tt roles ...`
- `tt worktrees`
- `tt app-server ...`
- `tt lane ...`
- `tt tui`
- `tt supervisor ...`
- `tt tt ...`
- `tt prompt ...`
- `tt quickstart ...`

## Daemon

- `tt daemon start`
- `tt daemon status`
- `tt daemon restart`
- `tt daemon stop`

## Doctor

- `tt doctor`

`tt doctor` now also reports the discovered lane roots under `~/.tt/lanes/` and prints the rendered lane manifest fields for each lane it finds.

## App Server

- `tt app-server add [<NAME>]`
- `tt app-server remove [<NAME>]`
- `tt app-server start [<NAME>]`
- `tt app-server stop [<NAME>]`
- `tt app-server restart [<NAME>]`
- `tt app-server status [<NAME>]`
- `tt app-server info [<NAME>]`

## Lane

- `tt lane init <LABEL> [--repo <ORG/REPO[=URL]>...]`
- `tt lane inspect <LABEL>`
- `tt lane attach <LABEL> --repo <ORG/REPO> [--workspace <NAME>] --tracked-thread <ID>`
- `tt lane detach <LABEL> --repo <ORG/REPO> [--workspace <NAME>] --tracked-thread <ID>`
- `tt lane cleanup <LABEL> [--repo <ORG/REPO[=URL]>] [--workspace <NAME>] [--scope <runtime|worktree|repo|lane>]`

Lane init renders a lane root under `~/.tt/lanes/<lane-slug>/`, seeds shared read-only home overlays, clones the requested repos, and creates a default workspace per repo. Lane cleanup is explicit and does not auto-garbage-collect inactive worktrees.
Lane attach and detach update the tracked-thread binding state through the authority store and mirror the attached tracked-thread ids into the workspace manifest on disk.

## Events

- `tt events recent [--limit <N>]`
- `tt events watch [--snapshot] [--count <N>]`

## Workstreams

- `tt workstream add <REPO_ROOT> <NAME>`
- `tt workstreams create --title <TEXT> --objective <TEXT> [--priority <TEXT>]`
- `tt workstreams edit --workstream <ID> [--title <TEXT>] [--objective <TEXT>] [--status <active|blocked|completed>] [--priority <TEXT>]`
- `tt workstream delete <WORKSTREAM>`
- `tt workstreams list`
- `tt workstreams get --workstream <ID>`

`tt workstream add` and `tt tt spawn --new-workstream` generate `worktree/<slug>` branch names by default and create worktree directories under the configured worktree root. The default root is `~/worktrees/tt`.
`tt app-server add default` and `tt app-server start default` refresh the managed `.tt/` template into the shared app-server `RUNTIME_HOME`.
`tt workstream delete` deletes the authority record only. Use `tt tt worktree prune <SELECTOR>` when you want to delete the branch, prune the git worktree, and delete the authority record in one lane-oriented operation.
`tt tt worktree prune` is the atomic lane cleanup path; `tt workstream delete` is the authority-only path.

## Roles

- `tt roles list`
- `tt roles info <ROLE>`

## Worktrees

- `tt worktrees`

## Workunit

- `tt workunit create --workstream <ID> --title <TEXT> --task <TEXT> [--dependency <ID>...]`
- `tt workunit edit --workunit <ID> [--title <TEXT>] [--task <TEXT>] [--status <ready|blocked|running|awaiting-decision|accepted|needs-human|completed>]`
- `tt workunit delete --workunit <ID>`
- `tt workunit list [--workstream <ID>]`
- `tt workunit get --workunit <ID>`

### Workunit Thread

- `tt workunit thread add --workunit <ID> --title <TEXT> --root-dir <PATH> [--notes <TEXT>] [--upstream-thread <ID>] [--model <MODEL>] [workspace flags...]`
- `tt workunit thread set --tracked-thread <ID> [--title <TEXT>] [--root-dir <PATH>] [--notes <TEXT>] [--upstream-thread <ID>] [--binding-state <unbound|bound|detached|missing>] [--model <MODEL>] [workspace flags...]`
- `tt workunit thread remove --tracked-thread <ID> [--request-note <TEXT>]`
- `tt workunit thread list --workunit <ID>`
- `tt workunit thread get --tracked-thread <ID> [--request-note <TEXT>]`

### Workunit Workspace

- `tt workunit workspace prepare-workspace --tracked-thread <ID> [--request-note <TEXT>]`
- `tt workunit workspace refresh-workspace --tracked-thread <ID> [--request-note <TEXT>]`
- `tt workunit workspace merge-prep --tracked-thread <ID> [--request-note <TEXT>]`
- `tt workunit workspace authorize-merge --tracked-thread <ID> [--request-note <TEXT>]`
- `tt workunit workspace execute-landing --tracked-thread <ID> [--request-note <TEXT>]`
- `tt workunit workspace prune-workspace --tracked-thread <ID> [--request-note <TEXT>]`

## Supervisor

- `tt supervisor session active`

### Supervisor Plan

- `tt supervisor plan create --workstream <ID> [--planning-thread <ID>] [planning summary flags...] [--created-by <TEXT>] [--request-note <TEXT>] [--model <MODEL>] [--cwd <PATH>]`
- `tt supervisor plan get --session <ID>`
- `tt supervisor plan list [--workstream <ID>] [--include-closed]`
- `tt supervisor plan update-summary --session <ID> [planning summary flags...] [--updated-by <TEXT>] [--note <TEXT>]`
- `tt supervisor plan request-supervisor-context --session <ID> [--requested-by <TEXT>] [--note <TEXT>]`
- `tt supervisor plan request-research --session <ID> --worker <ID> [--worker-kind <TEXT>] [--model <MODEL>] [--cwd <PATH>] [--requested-by <TEXT>] [--request-note <TEXT>]`
- `tt supervisor plan mark-ready-for-review --session <ID> [--updated-by <TEXT>] [--note <TEXT>]`
- `tt supervisor plan abort --session <ID> [--updated-by <TEXT>] [--note <TEXT>]`
- `tt supervisor plan approve --session <ID> [--approved-by <TEXT>] [--review-note <TEXT>]`
- `tt supervisor plan reject --session <ID> [--rejected-by <TEXT>] [--review-note <TEXT>]`
- `tt supervisor plan supersede --session <ID> [--superseded-by-session <ID>] [--updated-by <TEXT>] [--note <TEXT>]`

### Supervisor Work

- `tt supervisor work assignments start --workunit <ID> --worker <ID> [--instructions <TEXT>] [--worker-kind <TEXT>] [--cwd <PATH>] [--model <MODEL>]`
- `tt supervisor work assignments get --assignment <ID>`
- `tt supervisor work assignments communication --assignment <ID>`
- `tt supervisor work reports get --report <ID>`
- `tt supervisor work reports list-for-workunit --workunit <ID>`
- `tt supervisor work decisions apply --workunit <ID> --rationale <TEXT> --type <accept|continue|redirect|mark-complete|escalate-to-human> [--report <ID>] [--instructions <TEXT>] [--worker <ID>] [--worker-kind <TEXT>]`
- `tt supervisor work proposals create --workunit <ID> [--report <ID>] [--note <TEXT>] [--requested-by <TEXT>] [--supersede-open]`
- `tt supervisor work proposals get --proposal <ID>`
- `tt supervisor work proposals artifact-summary --proposal <ID>`
- `tt supervisor work proposals artifact-detail --proposal <ID>`
- `tt supervisor work proposals artifact-export --proposal <ID> [--format <json|md>] [--output <PATH>]`
- `tt supervisor work proposals list-for-workunit --workunit <ID>`
- `tt supervisor work proposals approve --proposal <ID> [--review-note <TEXT>] [--reviewed-by <TEXT>] [--type <accept|continue|redirect|mark-complete|escalate-to-human>] [--rationale <TEXT>] [--worker <ID>] [--worker-kind <TEXT>] [--objective <TEXT>] [--instruction <TEXT>...] [--acceptance <TEXT>...] [--stop-condition <TEXT>...] [--expected-report-field <TEXT>...]`
- `tt supervisor work proposals reject --proposal <ID> [--review-note <TEXT>] [--reviewed-by <TEXT>]`

### Supervisor Review

- `tt supervisor review list [review filters...] [--include-closed]`
- `tt supervisor review queue [review filters...]`
- `tt supervisor review history [--thread <ID>] [--assignment <ID>] [--include-superseded] [--limit <N>]`
- `tt supervisor review get --decision <ID>`
- `tt supervisor review propose-steer --thread <ID> --text <TEXT> [--requested-by <TEXT>] [--rationale-note <TEXT>]`
- `tt supervisor review replace-pending-steer --decision <ID> --text <TEXT> [--requested-by <TEXT>] [--rationale-note <TEXT>]`
- `tt supervisor review record-no-action --decision <ID> [--reviewed-by <TEXT>] [--review-note <TEXT>]`
- `tt supervisor review manual-refresh [--thread <ID>] [--assignment <ID>] [--requested-by <TEXT>] [--rationale-note <TEXT>]`
- `tt supervisor review approve --decision <ID> [--reviewed-by <TEXT>] [--review-note <TEXT>]`
- `tt supervisor review reject --decision <ID> [--reviewed-by <TEXT>] [--review-note <TEXT>]`

## TT

- `tt tt models list --workstream <ID>`
- `tt tt spawn <ROLE> [--workstream <SELECTOR> | --new-workstream <NAME> --repo-root <PATH>] [--headless] [--model <MODEL>]`
- `tt tt resume <THREAD> [--cwd <PATH>] [--model <MODEL>]`
- `tt tt worktree add <REPO_ROOT> <NAME>`
- `tt tt worktree prune <SELECTOR>`
- `tt tt threads list --workstream <ID>`
- `tt tt threads list-loaded --workstream <ID>`
- `tt tt threads read --thread <ID>`
- `tt tt threads start [--cwd <PATH>] [--model <MODEL>] [--ephemeral]`
- `tt tt threads resume --thread <ID> [--cwd <PATH>] [--model <MODEL>]`
- `tt tt turns list-active`
- `tt tt turns recent --thread <ID> [--limit <N>]`
- `tt tt turns get --thread <ID> --turn <ID>`

`tt tui` opens directly into a supervisor-backed TT session. It reuses a remembered supervisor thread when one exists for the current TT home, or creates one rooted at `~/.tt` when needed, then attaches the dashboard wrapper to that session. It launches the upstream TT TUI against the shared app-server with `tt --remote <ws-url> resume <THREAD>`, switchable tabs, and a border HUD with an explicit shortcut legend on its own visible line. The UI no longer renders left/right workstream or thread sidebars, and the startup canvas is the supervisor session rather than a blank screen; when the HUD is hidden, the canvas shows a `press F2 for HUD` hint.
Closing the TT dashboard exits only the wrapper; the launched TT TUI sessions are separate child processes and keep running until terminated directly.
Dashboard key bindings:

- `Ctrl+q` closes the TT wrapper only
- `F2` fades the HUD in and out
- `F5` refreshes live session status
- `F6` / `F7` switch between live TT session tabs
- `F8` terminates the active TT session

`tt tt worktree prune` accepts either a workstream selector or a tracked-thread id. It deletes the branch, prunes the worktree, and removes the corresponding authority record.

`threads list` and `threads read` include `management_state`, `owner_workstream_id`, and `runtime_workstream_id`.

- On shared runtimes, `threads list --workstream <ID>` is owner-scoped. It shows only threads TT has explicitly bound to that workstream.
- On dedicated runtimes, `threads list --workstream <ID>` can also show `observed_unmanaged` external threads that exist on that dedicated runtime but have not been adopted into TT.

## Workstream Runtimes

- `tt workstreams runtime list`
- `tt workstreams runtime get --workstream <ID>`
- `tt workstreams runtime start --workstream <ID>`
- `tt workstreams runtime stop --workstream <ID>`
- `tt workstreams runtime restart --workstream <ID>`

Dedicated runtime stop and restart are blocked while the runtime still exposes unmanaged external threads. TT only auto-retires an idle dedicated runtime when the runtime can be refreshed and reports zero observed threads.

## Prompt

- `tt prompt --thread <ID> --text <TEXT>`

## Quickstart

- `tt quickstart [--cwd <PATH>] [--model <MODEL>] --text <TEXT>`

## Remote

- `tt remote inbox list --origin <NODE_ID> [--source-kind <supervisor-proposal|supervisor-decision|planning-session|plan-revision-proposal>] [--actionable-only] [--include-closed] [--limit <N>]`
- `tt remote inbox get --origin <NODE_ID> --item <ITEM_ID>`
- `tt remote notifications list --origin <NODE_ID> [--status <pending|acknowledged|suppressed|obsolete>] [--pending-only] [--actionable-only] [--limit <N>]`
- `tt remote notifications get --origin <NODE_ID> --candidate <CANDIDATE_ID>`
- `tt remote notifications ack --origin <NODE_ID> --candidate <CANDIDATE_ID>`
- `tt remote notifications suppress --origin <NODE_ID> --candidate <CANDIDATE_ID>`
- `tt remote deliveries list [--origin <NODE_ID>] [--candidate <CANDIDATE_ID>] [--subscription <SUB_ID>] [--status <pending|dispatched|delivered|failed|suppressed|skipped|obsolete>] [--limit <N>]`
- `tt remote deliveries get --job <JOB_ID>`
- `tt remote actions submit --origin <NODE_ID> --item <ITEM_ID> --action <approve|reject|approve-and-send|record-no-action|manual-refresh|reconcile|retry|supersede|mark-ready-for-review> [--requested-by <TEXT>] [--note <TEXT>] [--idempotency-key <TEXT>]`
- `tt remote actions list --origin <NODE_ID> [--candidate <CANDIDATE_ID>] [--item <ITEM_ID>] [--action <approve|reject|approve-and-send|record-no-action|manual-refresh|reconcile|retry|supersede|mark-ready-for-review>] [--status <pending|claimed|completed|failed|canceled|stale>] [--pending-only] [--actionable-only] [--limit <N>]`
- `tt remote actions get --origin <NODE_ID> --request <REQUEST_ID>`
- `tt remote actions watch --origin <NODE_ID> --request <REQUEST_ID> [--timeout-ms <MS>]`

## Shared Argument Groups

- `planning summary flags`: `--objective`, `--requirement...`, `--constraint...`, `--non-goal...`, `--open-question...`, `--research-status`, `--draft-plan-summary`, `--ready-for-review`
- `review filters`: `--thread`, `--assignment`, `--workstream`, `--workunit`, `--supervisor`, `--status`, `--kind`, `--include-superseded`, `--limit`
- `workspace flags`: `--workspace-repository-root`, `--workspace-worktree-path`, `--workspace-branch-name`, `--workspace-base-ref`, `--workspace-base-commit`, `--workspace-landing-target`, `--workspace-strategy`, `--workspace-landing-policy`, `--workspace-sync-policy`, `--workspace-cleanup-policy`, `--workspace-status`, `--workspace-last-reported-head-commit`

If any workspace flag is used, the required minimum set is:

- `--workspace-repository-root`
- `--workspace-worktree-path`
- `--workspace-branch-name`
- `--workspace-base-ref`
- `--workspace-landing-target`
