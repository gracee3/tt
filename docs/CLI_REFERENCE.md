# Orcas CLI Reference

This file is the command-line reference for `orcas` as implemented in [crates/orcas/src/main.rs](/home/emmy/openai/orcas/crates/orcas/src/main.rs) and [crates/orcas/src/remote.rs](/home/emmy/openai/orcas/crates/orcas/src/remote.rs).

## Global Options

These flags are accepted before any subcommand.

- `--server-url <URL>`: operator server base URL. Env: `ORCAS_SERVER_URL`
- `--operator-api-token <TOKEN>`: bearer token for operator-server APIs. Env: `ORCAS_OPERATOR_API_TOKEN`
- `--codex-bin <PATH>`: override the local Codex binary path
- `--listen-url <WS_URL>`: override the upstream Codex app-server WebSocket URL
- `--inbox-mirror-server-url <URL>`: enable inbox mirroring to a server URL
- `--cwd <PATH>`: override the default working directory for the command
- `--model <MODEL>`: override the default model for the command
- `--connect-only`: require connect-only mode instead of spawning a local Codex app-server
- `--force-spawn`: force spawn mode instead of connect-only mode
- `-h, --help`
- `-V, --version`

## Top Level

- `orcas daemon ...`
- `orcas doctor`
- `orcas remote ...`
- `orcas events ...`
- `orcas workstreams ...`
- `orcas workunit ...`
- `orcas app-server ...`
- `orcas supervisor ...`
- `orcas codex ...`
- `orcas prompt ...`
- `orcas quickstart ...`

## Daemon

- `orcas daemon start`
- `orcas daemon status`
- `orcas daemon restart`
- `orcas daemon stop`

## Doctor

- `orcas doctor`

## App Server

- `orcas app-server list`
- `orcas app-server reap [--apply] [--all-tagged] [--include-untagged] [--pid <PID>...]`
- `orcas app-server info`

## Events

- `orcas events recent [--limit <N>]`
- `orcas events watch [--snapshot] [--count <N>]`

## Workstreams

- `orcas workstreams create --title <TEXT> --objective <TEXT> [--priority <TEXT>]`
- `orcas workstreams edit --workstream <ID> [--title <TEXT>] [--objective <TEXT>] [--status <active|blocked|completed>] [--priority <TEXT>]`
- `orcas workstreams delete --workstream <ID>`
- `orcas workstreams list`
- `orcas workstreams get --workstream <ID>`

## Workunit

- `orcas workunit create --workstream <ID> --title <TEXT> --task <TEXT> [--dependency <ID>...]`
- `orcas workunit edit --workunit <ID> [--title <TEXT>] [--task <TEXT>] [--status <ready|blocked|running|awaiting-decision|accepted|needs-human|completed>]`
- `orcas workunit delete --workunit <ID>`
- `orcas workunit list [--workstream <ID>]`
- `orcas workunit get --workunit <ID>`

### Workunit Thread

- `orcas workunit thread add --workunit <ID> --title <TEXT> --root-dir <PATH> [--notes <TEXT>] [--upstream-thread <ID>] [--model <MODEL>] [workspace flags...]`
- `orcas workunit thread set --tracked-thread <ID> [--title <TEXT>] [--root-dir <PATH>] [--notes <TEXT>] [--upstream-thread <ID>] [--binding-state <unbound|bound|detached|missing>] [--model <MODEL>] [workspace flags...]`
- `orcas workunit thread remove --tracked-thread <ID> [--request-note <TEXT>]`
- `orcas workunit thread list --workunit <ID>`
- `orcas workunit thread get --tracked-thread <ID> [--request-note <TEXT>]`

### Workunit Workspace

- `orcas workunit workspace prepare-workspace --tracked-thread <ID> [--request-note <TEXT>]`
- `orcas workunit workspace refresh-workspace --tracked-thread <ID> [--request-note <TEXT>]`
- `orcas workunit workspace merge-prep --tracked-thread <ID> [--request-note <TEXT>]`
- `orcas workunit workspace authorize-merge --tracked-thread <ID> [--request-note <TEXT>]`
- `orcas workunit workspace execute-landing --tracked-thread <ID> [--request-note <TEXT>]`
- `orcas workunit workspace prune-workspace --tracked-thread <ID> [--request-note <TEXT>]`

## Supervisor

- `orcas supervisor session active`

### Supervisor Plan

- `orcas supervisor plan create --workstream <ID> [--planning-thread <ID>] [planning summary flags...] [--created-by <TEXT>] [--request-note <TEXT>] [--model <MODEL>] [--cwd <PATH>]`
- `orcas supervisor plan get --session <ID>`
- `orcas supervisor plan list [--workstream <ID>] [--include-closed]`
- `orcas supervisor plan update-summary --session <ID> [planning summary flags...] [--updated-by <TEXT>] [--note <TEXT>]`
- `orcas supervisor plan request-supervisor-context --session <ID> [--requested-by <TEXT>] [--note <TEXT>]`
- `orcas supervisor plan request-research --session <ID> --worker <ID> [--worker-kind <TEXT>] [--model <MODEL>] [--cwd <PATH>] [--requested-by <TEXT>] [--request-note <TEXT>]`
- `orcas supervisor plan mark-ready-for-review --session <ID> [--updated-by <TEXT>] [--note <TEXT>]`
- `orcas supervisor plan abort --session <ID> [--updated-by <TEXT>] [--note <TEXT>]`
- `orcas supervisor plan approve --session <ID> [--approved-by <TEXT>] [--review-note <TEXT>]`
- `orcas supervisor plan reject --session <ID> [--rejected-by <TEXT>] [--review-note <TEXT>]`
- `orcas supervisor plan supersede --session <ID> [--superseded-by-session <ID>] [--updated-by <TEXT>] [--note <TEXT>]`

### Supervisor Work

- `orcas supervisor work assignments start --workunit <ID> --worker <ID> [--instructions <TEXT>] [--worker-kind <TEXT>] [--cwd <PATH>] [--model <MODEL>]`
- `orcas supervisor work assignments get --assignment <ID>`
- `orcas supervisor work assignments communication --assignment <ID>`
- `orcas supervisor work reports get --report <ID>`
- `orcas supervisor work reports list-for-workunit --workunit <ID>`
- `orcas supervisor work decisions apply --workunit <ID> --rationale <TEXT> --type <accept|continue|redirect|mark-complete|escalate-to-human> [--report <ID>] [--instructions <TEXT>] [--worker <ID>] [--worker-kind <TEXT>]`
- `orcas supervisor work proposals create --workunit <ID> [--report <ID>] [--note <TEXT>] [--requested-by <TEXT>] [--supersede-open]`
- `orcas supervisor work proposals get --proposal <ID>`
- `orcas supervisor work proposals artifact-summary --proposal <ID>`
- `orcas supervisor work proposals artifact-detail --proposal <ID>`
- `orcas supervisor work proposals artifact-export --proposal <ID> [--format <json|md>] [--output <PATH>]`
- `orcas supervisor work proposals list-for-workunit --workunit <ID>`
- `orcas supervisor work proposals approve --proposal <ID> [--review-note <TEXT>] [--reviewed-by <TEXT>] [--type <accept|continue|redirect|mark-complete|escalate-to-human>] [--rationale <TEXT>] [--worker <ID>] [--worker-kind <TEXT>] [--objective <TEXT>] [--instruction <TEXT>...] [--acceptance <TEXT>...] [--stop-condition <TEXT>...] [--expected-report-field <TEXT>...]`
- `orcas supervisor work proposals reject --proposal <ID> [--review-note <TEXT>] [--reviewed-by <TEXT>]`

### Supervisor Review

- `orcas supervisor review list [review filters...] [--include-closed]`
- `orcas supervisor review queue [review filters...]`
- `orcas supervisor review history [--thread <ID>] [--assignment <ID>] [--include-superseded] [--limit <N>]`
- `orcas supervisor review get --decision <ID>`
- `orcas supervisor review propose-steer --thread <ID> --text <TEXT> [--requested-by <TEXT>] [--rationale-note <TEXT>]`
- `orcas supervisor review replace-pending-steer --decision <ID> --text <TEXT> [--requested-by <TEXT>] [--rationale-note <TEXT>]`
- `orcas supervisor review record-no-action --decision <ID> [--reviewed-by <TEXT>] [--review-note <TEXT>]`
- `orcas supervisor review manual-refresh [--thread <ID>] [--assignment <ID>] [--requested-by <TEXT>] [--rationale-note <TEXT>]`
- `orcas supervisor review approve --decision <ID> [--reviewed-by <TEXT>] [--review-note <TEXT>]`
- `orcas supervisor review reject --decision <ID> [--reviewed-by <TEXT>] [--review-note <TEXT>]`

## Codex

- `orcas codex models list --workstream <ID>`
- `orcas codex threads list --workstream <ID>`
- `orcas codex threads list-loaded --workstream <ID>`
- `orcas codex threads read --thread <ID>`
- `orcas codex threads start [--cwd <PATH>] [--model <MODEL>] [--ephemeral]`
- `orcas codex threads resume --thread <ID> [--cwd <PATH>] [--model <MODEL>]`
- `orcas codex turns list-active`
- `orcas codex turns recent --thread <ID> [--limit <N>]`
- `orcas codex turns get --thread <ID> --turn <ID>`

## Workstream Runtimes

- `orcas workstreams runtime list`
- `orcas workstreams runtime get --workstream <ID>`
- `orcas workstreams runtime start --workstream <ID>`
- `orcas workstreams runtime stop --workstream <ID>`
- `orcas workstreams runtime restart --workstream <ID>`

## Prompt

- `orcas prompt --thread <ID> --text <TEXT>`

## Quickstart

- `orcas quickstart [--cwd <PATH>] [--model <MODEL>] --text <TEXT>`

## Remote

- `orcas remote inbox list --origin <NODE_ID> [--source-kind <supervisor-proposal|supervisor-decision|planning-session|plan-revision-proposal>] [--actionable-only] [--include-closed] [--limit <N>]`
- `orcas remote inbox get --origin <NODE_ID> --item <ITEM_ID>`
- `orcas remote notifications list --origin <NODE_ID> [--status <pending|acknowledged|suppressed|obsolete>] [--pending-only] [--actionable-only] [--limit <N>]`
- `orcas remote notifications get --origin <NODE_ID> --candidate <CANDIDATE_ID>`
- `orcas remote notifications ack --origin <NODE_ID> --candidate <CANDIDATE_ID>`
- `orcas remote notifications suppress --origin <NODE_ID> --candidate <CANDIDATE_ID>`
- `orcas remote deliveries list [--origin <NODE_ID>] [--candidate <CANDIDATE_ID>] [--subscription <SUB_ID>] [--status <pending|dispatched|delivered|failed|suppressed|skipped|obsolete>] [--limit <N>]`
- `orcas remote deliveries get --job <JOB_ID>`
- `orcas remote actions submit --origin <NODE_ID> --item <ITEM_ID> --action <approve|reject|approve-and-send|record-no-action|manual-refresh|reconcile|retry|supersede|mark-ready-for-review> [--requested-by <TEXT>] [--note <TEXT>] [--idempotency-key <TEXT>]`
- `orcas remote actions list --origin <NODE_ID> [--candidate <CANDIDATE_ID>] [--item <ITEM_ID>] [--action <approve|reject|approve-and-send|record-no-action|manual-refresh|reconcile|retry|supersede|mark-ready-for-review>] [--status <pending|claimed|completed|failed|canceled|stale>] [--pending-only] [--actionable-only] [--limit <N>]`
- `orcas remote actions get --origin <NODE_ID> --request <REQUEST_ID>`
- `orcas remote actions watch --origin <NODE_ID> --request <REQUEST_ID> [--timeout-ms <MS>]`

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
