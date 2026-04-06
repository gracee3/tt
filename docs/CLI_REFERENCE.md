# TT CLI Reference

Generated from the live `tt` Clap tree. Regenerate with `tt docs export-cli --out docs/CLI_REFERENCE.md`.

## `tt`

```text
tt control plane

Usage: tt [OPTIONS] <COMMAND>

Commands:
  daemon      Launch and manage the tt daemon
  doctor      
  docs        Export rendered CLI documentation
  remote      
  events      
  project     Manage durable tt project records
  worktree    Canonical authority-backed CRUD for planning work units
  roles       Inspect tt role definitions
  worktrees   
  app-server  Manage the shared tt app-server lifecycle
  lane        Manage lane-local runtimes and rendered directory state
  tui         Open the tt dashboard TUI
  supervisor  
  app         
  i3          
  skill       Run a typed skill runtime command
  prompt      
  quickstart  
  help        Print this message or the help of the given subcommand(s)

Options:
      --server-url <SERVER_URL>
          Base URL for the operator server
          
          [env: TT_SERVER_URL=]

      --operator-api-token <OPERATOR_API_TOKEN>
          Bearer token for operator-server APIs
          
          [env: TT_OPERATOR_API_TOKEN=]

      --tt-bin <TT_BIN>
          Override the local TT binary path for this command

      --listen-url <LISTEN_URL>
          Override the upstream TT app-server WebSocket URL

      --inbox-mirror-server-url <INBOX_MIRROR_SERVER_URL>
          Enable inbox mirroring to a server URL

      --cwd <CWD>
          Override the default working directory for this command

      --worktree-root <WORKTREE_ROOT>
          Override the default worktree root for project and TT spawn commands

      --model <MODEL>
          Override the default model for this command

      --connect-only
          Require attach-only mode for this process

      --force-spawn
          Legacy runtime override for spawn-capable processes

  -h, --help
          Print help

  -V, --version
          Print version
```

### `tt daemon`

```text
Launch and manage the tt daemon

Usage: daemon <COMMAND>

Commands:
  start    
  status   
  restart  
  stop     
  help     Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

#### `tt daemon start`

```text
Usage: start

Options:
  -h, --help
          Print help
```

#### `tt daemon status`

```text
Usage: status

Options:
  -h, --help
          Print help
```

#### `tt daemon restart`

```text
Usage: restart

Options:
  -h, --help
          Print help
```

#### `tt daemon stop`

```text
Usage: stop

Options:
  -h, --help
          Print help
```

### `tt doctor`

```text
Usage: doctor

Options:
  -h, --help
          Print help
```

### `tt docs`

```text
Export rendered CLI documentation

Usage: docs <COMMAND>

Commands:
  export-cli  
  help        Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

#### `tt docs export-cli`

```text
Usage: export-cli [OPTIONS]

Options:
      --out <OUT>
          Write the generated CLI reference to this file
          
          [default: docs/CLI_REFERENCE.md]

  -h, --help
          Print help
```

### `tt remote`

```text
Usage: remote <COMMAND>

Commands:
  inbox          
  notifications  
  deliveries     
  actions        
  help           Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

#### `tt remote inbox`

```text
Usage: inbox <COMMAND>

Commands:
  list  
  get   
  help  Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

##### `tt remote inbox list`

```text
Usage: list [OPTIONS] --origin <ORIGIN_NODE_ID>

Options:
      --origin <ORIGIN_NODE_ID>
          

      --source-kind <SOURCE_KIND>
          [possible values: supervisor-proposal, supervisor-decision, planning-session, plan-revision-proposal]

      --actionable-only
          

      --include-closed
          

      --limit <LIMIT>
          

  -h, --help
          Print help
```

##### `tt remote inbox get`

```text
Usage: get --origin <ORIGIN_NODE_ID> --item <ITEM_ID>

Options:
      --origin <ORIGIN_NODE_ID>
          

      --item <ITEM_ID>
          

  -h, --help
          Print help
```

#### `tt remote notifications`

```text
Usage: notifications <COMMAND>

Commands:
  list      
  get       
  ack       
  suppress  
  help      Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

##### `tt remote notifications list`

```text
Usage: list [OPTIONS] --origin <ORIGIN_NODE_ID>

Options:
      --origin <ORIGIN_NODE_ID>
          

      --status <STATUS>
          [possible values: pending, acknowledged, suppressed, obsolete]

      --pending-only
          

      --actionable-only
          

      --limit <LIMIT>
          

  -h, --help
          Print help
```

##### `tt remote notifications get`

```text
Usage: get --origin <ORIGIN_NODE_ID> --candidate <CANDIDATE_ID>

Options:
      --origin <ORIGIN_NODE_ID>
          

      --candidate <CANDIDATE_ID>
          

  -h, --help
          Print help
```

##### `tt remote notifications ack`

```text
Usage: ack --origin <ORIGIN_NODE_ID> --candidate <CANDIDATE_ID>

Options:
      --origin <ORIGIN_NODE_ID>
          

      --candidate <CANDIDATE_ID>
          

  -h, --help
          Print help
```

##### `tt remote notifications suppress`

```text
Usage: suppress --origin <ORIGIN_NODE_ID> --candidate <CANDIDATE_ID>

Options:
      --origin <ORIGIN_NODE_ID>
          

      --candidate <CANDIDATE_ID>
          

  -h, --help
          Print help
```

#### `tt remote deliveries`

```text
Usage: deliveries <COMMAND>

Commands:
  list  
  get   
  help  Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

##### `tt remote deliveries list`

```text
Usage: list [OPTIONS]

Options:
      --origin <ORIGIN_NODE_ID>
          

      --candidate <CANDIDATE_ID>
          

      --subscription <SUBSCRIPTION_ID>
          

      --status <STATUS>
          [possible values: pending, dispatched, delivered, failed, suppressed, skipped, obsolete]

      --limit <LIMIT>
          

  -h, --help
          Print help
```

##### `tt remote deliveries get`

```text
Usage: get --job <JOB_ID>

Options:
      --job <JOB_ID>
          

  -h, --help
          Print help
```

#### `tt remote actions`

```text
Usage: actions <COMMAND>

Commands:
  submit  
  list    
  get     
  watch   
  help    Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

##### `tt remote actions submit`

```text
Usage: submit [OPTIONS] --origin <ORIGIN_NODE_ID> --item <ITEM_ID> --action <ACTION_KIND>

Options:
      --origin <ORIGIN_NODE_ID>
          

      --item <ITEM_ID>
          

      --action <ACTION_KIND>
          [possible values: approve, reject, approve-and-send, record-no-action, manual-refresh, reconcile, retry, supersede, mark-ready-for-review]

      --requested-by <REQUESTED_BY>
          

      --note <REQUEST_NOTE>
          

      --idempotency-key <IDEMPOTENCY_KEY>
          

  -h, --help
          Print help
```

##### `tt remote actions list`

```text
Usage: list [OPTIONS] --origin <ORIGIN_NODE_ID>

Options:
      --origin <ORIGIN_NODE_ID>
          

      --candidate <CANDIDATE_ID>
          

      --item <ITEM_ID>
          

      --action <ACTION_KIND>
          [possible values: approve, reject, approve-and-send, record-no-action, manual-refresh, reconcile, retry, supersede, mark-ready-for-review]

      --status <STATUS>
          [possible values: pending, claimed, completed, failed, canceled, stale]

      --pending-only
          

      --actionable-only
          

      --limit <LIMIT>
          

  -h, --help
          Print help
```

##### `tt remote actions get`

```text
Usage: get --origin <ORIGIN_NODE_ID> --request <REQUEST_ID>

Options:
      --origin <ORIGIN_NODE_ID>
          

      --request <REQUEST_ID>
          

  -h, --help
          Print help
```

##### `tt remote actions watch`

```text
Usage: watch [OPTIONS] --origin <ORIGIN_NODE_ID> --request <REQUEST_ID>

Options:
      --origin <ORIGIN_NODE_ID>
          

      --request <REQUEST_ID>
          

      --timeout-ms <TIMEOUT_MS>
          

  -h, --help
          Print help
```

### `tt events`

```text
Usage: events <COMMAND>

Commands:
  recent  
  watch   
  help    Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

#### `tt events recent`

```text
Usage: recent [OPTIONS]

Options:
      --limit <LIMIT>
          [default: 20]

  -h, --help
          Print help
```

#### `tt events watch`

```text
Usage: watch [OPTIONS]

Options:
      --snapshot
          

      --count <COUNT>
          

  -h, --help
          Print help
```

### `tt project`

```text
Manage durable tt project records

Usage: project <COMMAND>

Commands:
  add     
  create  
  edit    
  delete  
  list    
  get     
  help    Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

#### `tt project add`

```text
Usage: add <REPO_ROOT> <NAME>

Arguments:
  <REPO_ROOT>
          

  <NAME>
          

Options:
  -h, --help
          Print help
```

#### `tt project create`

```text
Usage: create [OPTIONS] --title <TITLE> --objective <OBJECTIVE>

Options:
      --title <TITLE>
          

      --objective <OBJECTIVE>
          

      --priority <PRIORITY>
          

      --tt-home <TT_HOME>
          

      --sqlite-home <SQLITE_HOME>
          

      --listen-url <LISTEN_URL>
          

      --transport-kind <TRANSPORT_KIND>
          [possible values: local-app-server, remote-websocket]

      --app-server-policy <APP_SERVER_POLICY>
          [possible values: shared-current-daemon, dedicated-per-workstream]

      --connection-mode <CONNECTION_MODE>
          [possible values: connect-only, spawn-if-needed, spawn-always]

  -h, --help
          Print help
```

#### `tt project edit`

```text
Usage: edit [OPTIONS] --workstream <WORKSTREAM>

Options:
      --workstream <WORKSTREAM>
          

      --title <TITLE>
          

      --objective <OBJECTIVE>
          

      --status <STATUS>
          [possible values: active, blocked, completed]

      --priority <PRIORITY>
          

      --tt-home <TT_HOME>
          

      --sqlite-home <SQLITE_HOME>
          

      --listen-url <LISTEN_URL>
          

      --transport-kind <TRANSPORT_KIND>
          [possible values: local-app-server, remote-websocket]

      --app-server-policy <APP_SERVER_POLICY>
          [possible values: shared-current-daemon, dedicated-per-workstream]

      --connection-mode <CONNECTION_MODE>
          [possible values: connect-only, spawn-if-needed, spawn-always]

      --clear-execution-scope
          

  -h, --help
          Print help
```

#### `tt project delete`

```text
Usage: delete <WORKSTREAM>

Arguments:
  <WORKSTREAM>
          

Options:
  -h, --help
          Print help
```

#### `tt project list`

```text
Usage: list

Options:
  -h, --help
          Print help
```

#### `tt project get`

```text
Usage: get --workstream <WORKSTREAM>

Options:
      --workstream <WORKSTREAM>
          

  -h, --help
          Print help
```

### `tt worktree`

```text
Canonical authority-backed CRUD for planning work units

Usage: worktree <COMMAND>

Commands:
  create     
  edit       
  delete     
  list       
  get        
  thread     Canonical authority-backed CRUD for tracked-thread planning records
  workspace  Workspace operations for tracked-thread planning records
  help       Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

#### `tt worktree create`

```text
Usage: create [OPTIONS] --workstream <WORKSTREAM> --title <TITLE> --task <TASK>

Options:
      --workstream <WORKSTREAM>
          

      --title <TITLE>
          

      --task <TASK>
          

      --dependency <DEPENDENCIES>
          

  -h, --help
          Print help
```

#### `tt worktree edit`

```text
Usage: edit [OPTIONS] --workunit <WORKUNIT>

Options:
      --workunit <WORKUNIT>
          

      --title <TITLE>
          

      --task <TASK>
          

      --status <STATUS>
          [possible values: ready, blocked, running, awaiting-decision, accepted, needs-human, completed]

  -h, --help
          Print help
```

#### `tt worktree delete`

```text
Usage: delete --workunit <WORKUNIT>

Options:
      --workunit <WORKUNIT>
          

  -h, --help
          Print help
```

#### `tt worktree list`

```text
Usage: list [OPTIONS]

Options:
      --workstream <WORKSTREAM>
          

  -h, --help
          Print help
```

#### `tt worktree get`

```text
Usage: get --workunit <WORKUNIT>

Options:
      --workunit <WORKUNIT>
          

  -h, --help
          Print help
```

#### `tt worktree thread`

```text
Canonical authority-backed CRUD for tracked-thread planning records

Usage: thread <COMMAND>

Commands:
  add     
  set     
  remove  
  list    
  get     
  help    Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

##### `tt worktree thread add`

```text
Usage: add [OPTIONS] --workunit <WORKUNIT> --title <TITLE> --root-dir <ROOT_DIR>

Options:
      --workunit <WORKUNIT>
          

      --title <TITLE>
          

      --root-dir <ROOT_DIR>
          

      --notes <NOTES>
          

      --upstream-thread <UPSTREAM_THREAD>
          

      --model <MODEL>
          

      --workspace-repository-root <REPOSITORY_ROOT>
          

      --workspace-worktree-path <WORKTREE_PATH>
          

      --workspace-branch-name <BRANCH_NAME>
          

      --workspace-base-ref <BASE_REF>
          

      --workspace-base-commit <BASE_COMMIT>
          

      --workspace-landing-target <LANDING_TARGET>
          

      --workspace-strategy <STRATEGY>
          [possible values: shared, dedicated-thread-worktree, ephemeral]

      --workspace-landing-policy <LANDING_POLICY>
          [possible values: merge-to-main, merge-to-campaign, cherry-pick-only, parked]

      --workspace-sync-policy <SYNC_POLICY>
          [possible values: manual, rebase-before-completion, rebase-before-each-assignment]

      --workspace-cleanup-policy <CLEANUP_POLICY>
          [possible values: keep-until-campaign-closed, prune-after-merge, keep-for-audit]

      --workspace-status <STATUS>
          [possible values: requested, ready, dirty, ahead, behind, conflicted, merged, abandoned, pruned]

      --workspace-last-reported-head-commit <LAST_REPORTED_HEAD_COMMIT>
          

  -h, --help
          Print help
```

##### `tt worktree thread set`

```text
Usage: set [OPTIONS] --tracked-thread <TRACKED_THREAD>

Options:
      --tracked-thread <TRACKED_THREAD>
          

      --title <TITLE>
          

      --root-dir <ROOT_DIR>
          

      --notes <NOTES>
          

      --upstream-thread <UPSTREAM_THREAD>
          

      --binding-state <BINDING_STATE>
          [possible values: unbound, bound, detached, missing]

      --model <MODEL>
          

      --workspace-repository-root <REPOSITORY_ROOT>
          

      --workspace-worktree-path <WORKTREE_PATH>
          

      --workspace-branch-name <BRANCH_NAME>
          

      --workspace-base-ref <BASE_REF>
          

      --workspace-base-commit <BASE_COMMIT>
          

      --workspace-landing-target <LANDING_TARGET>
          

      --workspace-strategy <STRATEGY>
          [possible values: shared, dedicated-thread-worktree, ephemeral]

      --workspace-landing-policy <LANDING_POLICY>
          [possible values: merge-to-main, merge-to-campaign, cherry-pick-only, parked]

      --workspace-sync-policy <SYNC_POLICY>
          [possible values: manual, rebase-before-completion, rebase-before-each-assignment]

      --workspace-cleanup-policy <CLEANUP_POLICY>
          [possible values: keep-until-campaign-closed, prune-after-merge, keep-for-audit]

      --workspace-status <STATUS>
          [possible values: requested, ready, dirty, ahead, behind, conflicted, merged, abandoned, pruned]

      --workspace-last-reported-head-commit <LAST_REPORTED_HEAD_COMMIT>
          

  -h, --help
          Print help
```

##### `tt worktree thread remove`

```text
Usage: remove [OPTIONS] --tracked-thread <TRACKED_THREAD>

Options:
      --tracked-thread <TRACKED_THREAD>
          

      --request-note <REQUEST_NOTE>
          

  -h, --help
          Print help
```

##### `tt worktree thread list`

```text
Usage: list --workunit <WORKUNIT>

Options:
      --workunit <WORKUNIT>
          

  -h, --help
          Print help
```

##### `tt worktree thread get`

```text
Usage: get [OPTIONS] --tracked-thread <TRACKED_THREAD>

Options:
      --tracked-thread <TRACKED_THREAD>
          

      --request-note <REQUEST_NOTE>
          

  -h, --help
          Print help
```

#### `tt worktree workspace`

```text
Workspace operations for tracked-thread planning records

Usage: workspace <COMMAND>

Commands:
  prepare-workspace  
  refresh-workspace  
  merge-prep         
  authorize-merge    
  execute-landing    
  prune-workspace    
  help               Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

##### `tt worktree workspace prepare-workspace`

```text
Usage: prepare-workspace [OPTIONS] --tracked-thread <TRACKED_THREAD>

Options:
      --tracked-thread <TRACKED_THREAD>
          

      --request-note <REQUEST_NOTE>
          

  -h, --help
          Print help
```

##### `tt worktree workspace refresh-workspace`

```text
Usage: refresh-workspace [OPTIONS] --tracked-thread <TRACKED_THREAD>

Options:
      --tracked-thread <TRACKED_THREAD>
          

      --request-note <REQUEST_NOTE>
          

  -h, --help
          Print help
```

##### `tt worktree workspace merge-prep`

```text
Usage: merge-prep [OPTIONS] --tracked-thread <TRACKED_THREAD>

Options:
      --tracked-thread <TRACKED_THREAD>
          

      --request-note <REQUEST_NOTE>
          

  -h, --help
          Print help
```

##### `tt worktree workspace authorize-merge`

```text
Usage: authorize-merge [OPTIONS] --tracked-thread <TRACKED_THREAD>

Options:
      --tracked-thread <TRACKED_THREAD>
          

      --request-note <REQUEST_NOTE>
          

  -h, --help
          Print help
```

##### `tt worktree workspace execute-landing`

```text
Usage: execute-landing [OPTIONS] --tracked-thread <TRACKED_THREAD>

Options:
      --tracked-thread <TRACKED_THREAD>
          

      --request-note <REQUEST_NOTE>
          

  -h, --help
          Print help
```

##### `tt worktree workspace prune-workspace`

```text
Usage: prune-workspace [OPTIONS] --tracked-thread <TRACKED_THREAD>

Options:
      --tracked-thread <TRACKED_THREAD>
          

      --request-note <REQUEST_NOTE>
          

  -h, --help
          Print help
```

### `tt roles`

```text
Inspect tt role definitions

Usage: roles <COMMAND>

Commands:
  list  
  info  
  help  Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

#### `tt roles list`

```text
Usage: list

Options:
  -h, --help
          Print help
```

#### `tt roles info`

```text
Usage: info <ROLE>

Arguments:
  <ROLE>
          

Options:
  -h, --help
          Print help
```

### `tt worktrees`

```text
Usage: worktrees

Options:
  -h, --help
          Print help
```

### `tt app-server`

```text
Manage the shared tt app-server lifecycle

Usage: app-server <COMMAND>

Commands:
  add      
  remove   
  start    
  stop     
  restart  
  status   
  info     
  help     Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

#### `tt app-server add`

```text
Usage: add [NAME]

Arguments:
  [NAME]
          [default: default]

Options:
  -h, --help
          Print help
```

#### `tt app-server remove`

```text
Usage: remove [NAME]

Arguments:
  [NAME]
          [default: default]

Options:
  -h, --help
          Print help
```

#### `tt app-server start`

```text
Usage: start [NAME]

Arguments:
  [NAME]
          [default: default]

Options:
  -h, --help
          Print help
```

#### `tt app-server stop`

```text
Usage: stop [NAME]

Arguments:
  [NAME]
          [default: default]

Options:
  -h, --help
          Print help
```

#### `tt app-server restart`

```text
Usage: restart [NAME]

Arguments:
  [NAME]
          [default: default]

Options:
  -h, --help
          Print help
```

#### `tt app-server status`

```text
Usage: status [NAME]

Arguments:
  [NAME]
          [default: default]

Options:
  -h, --help
          Print help
```

#### `tt app-server info`

```text
Usage: info [NAME]

Arguments:
  [NAME]
          [default: default]

Options:
  -h, --help
          Print help
```

### `tt lane`

```text
Manage lane-local runtimes and rendered directory state

Usage: lane <COMMAND>

Commands:
  list     List rendered lane roots and attachment counts
  init     
  inspect  
  attach   
  detach   
  cleanup  
  help     Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

#### `tt lane list`

```text
List rendered lane roots and attachment counts

Usage: list

Options:
  -h, --help
          Print help
```

#### `tt lane init`

```text
Usage: init [OPTIONS] <LABEL>

Arguments:
  <LABEL>
          

Options:
      --repo <REPOS>
          

  -h, --help
          Print help
```

#### `tt lane inspect`

```text
Usage: inspect <LABEL>

Arguments:
  <LABEL>
          

Options:
  -h, --help
          Print help
```

#### `tt lane attach`

```text
Usage: attach [OPTIONS] --repo <REPO> --tracked-thread <TRACKED_THREAD> <LABEL>

Arguments:
  <LABEL>
          

Options:
      --repo <REPO>
          

      --workspace <WORKSPACE>
          

      --tracked-thread <TRACKED_THREAD>
          

  -h, --help
          Print help
```

#### `tt lane detach`

```text
Usage: detach [OPTIONS] --repo <REPO> --tracked-thread <TRACKED_THREAD> <LABEL>

Arguments:
  <LABEL>
          

Options:
      --repo <REPO>
          

      --workspace <WORKSPACE>
          

      --tracked-thread <TRACKED_THREAD>
          

  -h, --help
          Print help
```

#### `tt lane cleanup`

```text
Usage: cleanup [OPTIONS] <LABEL>

Arguments:
  <LABEL>
          

Options:
      --repo <REPO>
          

      --workspace <WORKSPACE>
          

      --scope <SCOPE>
          [default: runtime]
          [possible values: runtime, worktree, repo, lane]

      --force
          

  -h, --help
          Print help
```

### `tt tui`

```text
Open the tt dashboard TUI

Usage: tui

Options:
  -h, --help
          Print help
```

### `tt supervisor`

```text
Usage: supervisor <COMMAND>

Commands:
  plan     Supervisor-owned planning session orchestration
  work     
  review   
  session  
  help     Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

#### `tt supervisor plan`

```text
Supervisor-owned planning session orchestration

Usage: plan <COMMAND>

Commands:
  create                      Create a draft planning session; readiness must be set later with mark-ready-for-review
  get                         
  list                        
  update-summary              Update the descriptive planning summary only; use mark-ready-for-review for readiness
  request-supervisor-context  Request more supervisor context while the session is still chatting
  request-research            Request the bounded one-turn research assignment for this session
  mark-ready-for-review       Explicitly transition a chat session into awaiting-approval
  abort                       Abort the planning session without mutating canonical plan state
  approve                     Stage a canonical plan revision proposal from the session summary
  reject                      Reject the planning session without mutating canonical plan state
  supersede                   Supersede the planning session without mutating canonical plan state
  help                        Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

##### `tt supervisor plan create`

```text
Create a draft planning session; readiness must be set later with mark-ready-for-review

Usage: create [OPTIONS] --workstream <WORKSTREAM> --objective <OBJECTIVE>

Options:
      --workstream <WORKSTREAM>
          

      --planning-thread <PLANNING_THREAD_ID>
          

      --objective <OBJECTIVE>
          

      --requirement <REQUIREMENTS>
          

      --constraint <CONSTRAINTS>
          

      --non-goal <NON_GOALS>
          

      --open-question <OPEN_QUESTIONS>
          

      --research-status <RESEARCH_STATUS>
          [default: not-requested]
          [possible values: not-requested, requested, completed, failed]

      --draft-plan-summary <DRAFT_PLAN_SUMMARY>
          

      --ready-for-review
          Reserved for the explicit mark-ready-for-review transition; create/update should leave this false

      --created-by <CREATED_BY>
          

      --request-note <REQUEST_NOTE>
          

      --model <MODEL>
          

      --cwd <CWD>
          

  -h, --help
          Print help
```

##### `tt supervisor plan get`

```text
Usage: get --session <SESSION>

Options:
      --session <SESSION>
          

  -h, --help
          Print help
```

##### `tt supervisor plan list`

```text
Usage: list [OPTIONS]

Options:
      --workstream <WORKSTREAM>
          

      --include-closed
          

  -h, --help
          Print help
```

##### `tt supervisor plan update-summary`

```text
Update the descriptive planning summary only; use mark-ready-for-review for readiness

Usage: update-summary [OPTIONS] --session <SESSION> --objective <OBJECTIVE>

Options:
      --session <SESSION>
          

      --objective <OBJECTIVE>
          

      --requirement <REQUIREMENTS>
          

      --constraint <CONSTRAINTS>
          

      --non-goal <NON_GOALS>
          

      --open-question <OPEN_QUESTIONS>
          

      --research-status <RESEARCH_STATUS>
          [default: not-requested]
          [possible values: not-requested, requested, completed, failed]

      --draft-plan-summary <DRAFT_PLAN_SUMMARY>
          

      --ready-for-review
          Reserved for the explicit mark-ready-for-review transition; create/update should leave this false

      --updated-by <UPDATED_BY>
          

      --note <NOTE>
          

  -h, --help
          Print help
```

##### `tt supervisor plan request-supervisor-context`

```text
Request more supervisor context while the session is still chatting

Usage: request-supervisor-context [OPTIONS] --session <SESSION>

Options:
      --session <SESSION>
          

      --requested-by <REQUESTED_BY>
          

      --note <NOTE>
          

  -h, --help
          Print help
```

##### `tt supervisor plan request-research`

```text
Request the bounded one-turn research assignment for this session

Usage: request-research [OPTIONS] --session <SESSION> --worker <WORKER>

Options:
      --session <SESSION>
          

      --worker <WORKER>
          

      --worker-kind <WORKER_KIND>
          

      --model <MODEL>
          

      --cwd <CWD>
          

      --requested-by <REQUESTED_BY>
          

      --request-note <REQUEST_NOTE>
          

  -h, --help
          Print help
```

##### `tt supervisor plan mark-ready-for-review`

```text
Explicitly transition a chat session into awaiting-approval

Usage: mark-ready-for-review [OPTIONS] --session <SESSION>

Options:
      --session <SESSION>
          

      --updated-by <UPDATED_BY>
          

      --note <NOTE>
          

  -h, --help
          Print help
```

##### `tt supervisor plan abort`

```text
Abort the planning session without mutating canonical plan state

Usage: abort [OPTIONS] --session <SESSION>

Options:
      --session <SESSION>
          

      --updated-by <UPDATED_BY>
          

      --note <NOTE>
          

  -h, --help
          Print help
```

##### `tt supervisor plan approve`

```text
Stage a canonical plan revision proposal from the session summary

Usage: approve [OPTIONS] --session <SESSION>

Options:
      --session <SESSION>
          

      --approved-by <APPROVED_BY>
          

      --review-note <REVIEW_NOTE>
          

  -h, --help
          Print help
```

##### `tt supervisor plan reject`

```text
Reject the planning session without mutating canonical plan state

Usage: reject [OPTIONS] --session <SESSION>

Options:
      --session <SESSION>
          

      --rejected-by <REJECTED_BY>
          

      --review-note <REVIEW_NOTE>
          

  -h, --help
          Print help
```

##### `tt supervisor plan supersede`

```text
Supersede the planning session without mutating canonical plan state

Usage: supersede [OPTIONS] --session <SESSION>

Options:
      --session <SESSION>
          

      --superseded-by-session <SUPERSEDED_BY_SESSION>
          

      --updated-by <UPDATED_BY>
          

      --note <NOTE>
          

  -h, --help
          Print help
```

#### `tt supervisor work`

```text
Usage: work <COMMAND>

Commands:
  assignments  
  reports      
  decisions    
  proposals    
  help         Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

##### `tt supervisor work assignments`

```text
Usage: assignments <COMMAND>

Commands:
  start          
  get            
  communication  
  help           Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

###### `tt supervisor work assignments start`

```text
Usage: start [OPTIONS] --workunit <WORKUNIT> --worker <WORKER>

Options:
      --workunit <WORKUNIT>
          

      --worker <WORKER>
          

      --instructions <INSTRUCTIONS>
          

      --worker-kind <WORKER_KIND>
          

      --cwd <CWD>
          

      --model <MODEL>
          

  -h, --help
          Print help
```

###### `tt supervisor work assignments get`

```text
Usage: get --assignment <ASSIGNMENT>

Options:
      --assignment <ASSIGNMENT>
          

  -h, --help
          Print help
```

###### `tt supervisor work assignments communication`

```text
Usage: communication --assignment <ASSIGNMENT>

Options:
      --assignment <ASSIGNMENT>
          

  -h, --help
          Print help
```

##### `tt supervisor work reports`

```text
Usage: reports <COMMAND>

Commands:
  get                
  list-for-workunit  
  help               Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

###### `tt supervisor work reports get`

```text
Usage: get --report <REPORT>

Options:
      --report <REPORT>
          

  -h, --help
          Print help
```

###### `tt supervisor work reports list-for-workunit`

```text
Usage: list-for-workunit --workunit <WORKUNIT>

Options:
      --workunit <WORKUNIT>
          

  -h, --help
          Print help
```

##### `tt supervisor work decisions`

```text
Usage: decisions <COMMAND>

Commands:
  apply  
  help   Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

###### `tt supervisor work decisions apply`

```text
Usage: apply [OPTIONS] --workunit <WORKUNIT> --rationale <RATIONALE> --type <DECISION_TYPE>

Options:
      --workunit <WORKUNIT>
          

      --rationale <RATIONALE>
          

      --report <REPORT>
          

      --type <DECISION_TYPE>
          [possible values: accept, continue, redirect, mark-complete, escalate-to-human]

      --instructions <INSTRUCTIONS>
          

      --worker <WORKER>
          

      --worker-kind <WORKER_KIND>
          

  -h, --help
          Print help
```

##### `tt supervisor work proposals`

```text
Usage: proposals <COMMAND>

Commands:
  create             
  get                
  artifact-summary   
  artifact-detail    
  artifact-export    
  list-for-workunit  
  approve            
  reject             
  help               Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

###### `tt supervisor work proposals create`

```text
Usage: create [OPTIONS] --workunit <WORKUNIT>

Options:
      --workunit <WORKUNIT>
          

      --report <REPORT>
          

      --note <NOTE>
          

      --requested-by <REQUESTED_BY>
          

      --supersede-open
          

  -h, --help
          Print help
```

###### `tt supervisor work proposals get`

```text
Usage: get --proposal <PROPOSAL>

Options:
      --proposal <PROPOSAL>
          

  -h, --help
          Print help
```

###### `tt supervisor work proposals artifact-summary`

```text
Usage: artifact-summary --proposal <PROPOSAL>

Options:
      --proposal <PROPOSAL>
          

  -h, --help
          Print help
```

###### `tt supervisor work proposals artifact-detail`

```text
Usage: artifact-detail --proposal <PROPOSAL>

Options:
      --proposal <PROPOSAL>
          

  -h, --help
          Print help
```

###### `tt supervisor work proposals artifact-export`

```text
Usage: artifact-export [OPTIONS] --proposal <PROPOSAL>

Options:
      --proposal <PROPOSAL>
          

      --format <FORMAT>
          [default: json]
          [possible values: json, md]

      --output <OUTPUT>
          

  -h, --help
          Print help
```

###### `tt supervisor work proposals list-for-workunit`

```text
Usage: list-for-workunit --workunit <WORKUNIT>

Options:
      --workunit <WORKUNIT>
          

  -h, --help
          Print help
```

###### `tt supervisor work proposals approve`

```text
Usage: approve [OPTIONS] --proposal <PROPOSAL>

Options:
      --proposal <PROPOSAL>
          

      --review-note <REVIEW_NOTE>
          

      --reviewed-by <REVIEWED_BY>
          

      --type <DECISION_TYPE>
          [possible values: accept, continue, redirect, mark-complete, escalate-to-human]

      --rationale <RATIONALE>
          

      --worker <WORKER>
          

      --worker-kind <WORKER_KIND>
          

      --objective <OBJECTIVE>
          

      --instruction <INSTRUCTIONS>
          

      --acceptance <ACCEPTANCE_CRITERIA>
          

      --stop-condition <STOP_CONDITIONS>
          

      --expected-report-field <EXPECTED_REPORT_FIELDS>
          

  -h, --help
          Print help
```

###### `tt supervisor work proposals reject`

```text
Usage: reject [OPTIONS] --proposal <PROPOSAL>

Options:
      --proposal <PROPOSAL>
          

      --review-note <REVIEW_NOTE>
          

      --reviewed-by <REVIEWED_BY>
          

  -h, --help
          Print help
```

#### `tt supervisor review`

```text
Usage: review <COMMAND>

Commands:
  list                   
  queue                  
  history                
  get                    
  propose-steer          
  replace-pending-steer  
  record-no-action       
  manual-refresh         
  approve                
  reject                 
  help                   Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

##### `tt supervisor review list`

```text
Usage: list [OPTIONS]

Options:
      --thread <THREAD>
          

      --assignment <ASSIGNMENT>
          

      --workstream <WORKSTREAM>
          

      --workunit <WORKUNIT>
          

      --supervisor <SUPERVISOR>
          

      --status <STATUS>
          [possible values: proposed-to-human, recorded, sent, rejected, stale, superseded]

      --kind <KIND>
          [possible values: next-turn, steer-active-turn, interrupt-active-turn, no-action]

      --include-superseded
          

      --limit <LIMIT>
          

      --include-closed
          

  -h, --help
          Print help
```

##### `tt supervisor review queue`

```text
Usage: queue [OPTIONS]

Options:
      --thread <THREAD>
          

      --assignment <ASSIGNMENT>
          

      --workstream <WORKSTREAM>
          

      --workunit <WORKUNIT>
          

      --supervisor <SUPERVISOR>
          

      --status <STATUS>
          [possible values: proposed-to-human, recorded, sent, rejected, stale, superseded]

      --kind <KIND>
          [possible values: next-turn, steer-active-turn, interrupt-active-turn, no-action]

      --include-superseded
          

      --limit <LIMIT>
          

  -h, --help
          Print help
```

##### `tt supervisor review history`

```text
Usage: history [OPTIONS]

Options:
      --thread <THREAD>
          

      --assignment <ASSIGNMENT>
          

      --include-superseded
          

      --limit <LIMIT>
          

  -h, --help
          Print help
```

##### `tt supervisor review get`

```text
Usage: get --decision <DECISION>

Options:
      --decision <DECISION>
          

  -h, --help
          Print help
```

##### `tt supervisor review propose-steer`

```text
Usage: propose-steer [OPTIONS] --thread <THREAD> --text <TEXT>

Options:
      --thread <THREAD>
          

      --text <TEXT>
          

      --requested-by <REQUESTED_BY>
          

      --rationale-note <RATIONALE_NOTE>
          

  -h, --help
          Print help
```

##### `tt supervisor review replace-pending-steer`

```text
Usage: replace-pending-steer [OPTIONS] --decision <DECISION> --text <TEXT>

Options:
      --decision <DECISION>
          

      --text <TEXT>
          

      --requested-by <REQUESTED_BY>
          

      --rationale-note <RATIONALE_NOTE>
          

  -h, --help
          Print help
```

##### `tt supervisor review record-no-action`

```text
Usage: record-no-action [OPTIONS] --decision <DECISION>

Options:
      --decision <DECISION>
          

      --reviewed-by <REVIEWED_BY>
          

      --review-note <REVIEW_NOTE>
          

  -h, --help
          Print help
```

##### `tt supervisor review manual-refresh`

```text
Usage: manual-refresh [OPTIONS]

Options:
      --thread <THREAD>
          

      --assignment <ASSIGNMENT>
          

      --requested-by <REQUESTED_BY>
          

      --rationale-note <RATIONALE_NOTE>
          

  -h, --help
          Print help
```

##### `tt supervisor review approve`

```text
Usage: approve [OPTIONS] --decision <DECISION>

Options:
      --decision <DECISION>
          

      --reviewed-by <REVIEWED_BY>
          

      --review-note <REVIEW_NOTE>
          

  -h, --help
          Print help
```

##### `tt supervisor review reject`

```text
Usage: reject [OPTIONS] --decision <DECISION>

Options:
      --decision <DECISION>
          

      --reviewed-by <REVIEWED_BY>
          

      --review-note <REVIEW_NOTE>
          

  -h, --help
          Print help
```

#### `tt supervisor session`

```text
Usage: session <COMMAND>

Commands:
  active  
  help    Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

##### `tt supervisor session active`

```text
Usage: active

Options:
  -h, --help
          Print help
```

### `tt app`

```text
Usage: app <COMMAND>

Commands:
  tt    
  help  Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

#### `tt app tt`

```text
Usage: tt <COMMAND>

Commands:
  models    
  spawn     
  resume    
  worktree  TT lane worktree lifecycle helpers
  threads   
  turns     
  help      Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

##### `tt app tt models`

```text
Usage: models <COMMAND>

Commands:
  list  
  help  Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

###### `tt app tt models list`

```text
Usage: list --workstream <WORKSTREAM>

Options:
      --workstream <WORKSTREAM>
          

  -h, --help
          Print help
```

##### `tt app tt spawn`

```text
Usage: spawn [OPTIONS] <ROLE>

Arguments:
  <ROLE>
          

Options:
      --workstream <WORKSTREAM>
          

      --new-workstream <NEW_WORKSTREAM>
          

      --repo-root <REPO_ROOT>
          

      --headless
          

      --model <MODEL>
          

  -h, --help
          Print help
```

##### `tt app tt resume`

```text
Usage: resume [OPTIONS] <THREAD>

Arguments:
  <THREAD>
          

Options:
      --cwd <CWD>
          

      --model <MODEL>
          

  -h, --help
          Print help
```

##### `tt app tt worktree`

```text
TT lane worktree lifecycle helpers

Usage: worktree <COMMAND>

Commands:
  add    
  prune  
  help   Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

###### `tt app tt worktree add`

```text
Usage: add <REPO_ROOT> <NAME>

Arguments:
  <REPO_ROOT>
          

  <NAME>
          

Options:
  -h, --help
          Print help
```

###### `tt app tt worktree prune`

```text
Usage: prune <SELECTOR>

Arguments:
  <SELECTOR>
          

Options:
  -h, --help
          Print help
```

##### `tt app tt threads`

```text
Usage: threads <COMMAND>

Commands:
  list         
  list-loaded  
  read         
  start        
  resume       
  help         Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

###### `tt app tt threads list`

```text
Usage: list --workstream <WORKSTREAM>

Options:
      --workstream <WORKSTREAM>
          

  -h, --help
          Print help
```

###### `tt app tt threads list-loaded`

```text
Usage: list-loaded --workstream <WORKSTREAM>

Options:
      --workstream <WORKSTREAM>
          

  -h, --help
          Print help
```

###### `tt app tt threads read`

```text
Usage: read --thread <THREAD>

Options:
      --thread <THREAD>
          

  -h, --help
          Print help
```

###### `tt app tt threads start`

```text
Usage: start [OPTIONS]

Options:
      --cwd <CWD>
          

      --model <MODEL>
          

      --ephemeral
          

  -h, --help
          Print help
```

###### `tt app tt threads resume`

```text
Usage: resume [OPTIONS] --thread <THREAD>

Options:
      --thread <THREAD>
          

      --cwd <CWD>
          

      --model <MODEL>
          

  -h, --help
          Print help
```

##### `tt app tt turns`

```text
Usage: turns <COMMAND>

Commands:
  list-active  
  recent       
  get          
  help         Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

###### `tt app tt turns list-active`

```text
Usage: list-active

Options:
  -h, --help
          Print help
```

###### `tt app tt turns recent`

```text
Usage: recent [OPTIONS] --thread <THREAD>

Options:
      --thread <THREAD>
          

      --limit <LIMIT>
          [default: 10]

  -h, --help
          Print help
```

###### `tt app tt turns get`

```text
Usage: get --thread <THREAD> --turn <TURN>

Options:
      --thread <THREAD>
          

      --turn <TURN>
          

  -h, --help
          Print help
```

### `tt i3`

```text
Usage: i3 <COMMAND>

Commands:
  status  
  start   
  attach  
  help    Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

#### `tt i3 status`

```text
Usage: status

Options:
  -h, --help
          Print help
```

#### `tt i3 start`

```text
Usage: start

Options:
  -h, --help
          Print help
```

#### `tt i3 attach`

```text
Usage: attach

Options:
  -h, --help
          Print help
```

### `tt skill`

```text
Run a typed skill runtime command

Usage: skill <COMMAND>

Commands:
  agent     
  i3        
  tt        
  process   
  services  
  git       
  help      Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

#### `tt skill agent`

```text
Usage: agent <COMMAND>

Commands:
  spawn    
  inspect  
  resume   
  retire   
  help     Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

##### `tt skill agent spawn`

```text
Usage: spawn [OPTIONS] [ROLE]

Arguments:
  [ROLE]
          [default: agent]

Options:
      --workstream <WORKSTREAM>
          

      --new-workstream <NEW_WORKSTREAM>
          

      --repo-root <REPO_ROOT>
          

      --headless
          

      --model <MODEL>
          

  -h, --help
          Print help
```

##### `tt skill agent inspect`

```text
Usage: inspect [OPTIONS]

Options:
      --thread <THREAD>
          

      --workstream <WORKSTREAM>
          

  -h, --help
          Print help
```

##### `tt skill agent resume`

```text
Usage: resume [OPTIONS] <THREAD>

Arguments:
  <THREAD>
          

Options:
      --cwd <CWD>
          

      --model <MODEL>
          

  -h, --help
          Print help
```

##### `tt skill agent retire`

```text
Usage: retire [OPTIONS] <THREAD>

Arguments:
  <THREAD>
          

Options:
      --note <NOTE>
          

  -h, --help
          Print help
```

#### `tt skill i3`

```text
Usage: i3 <COMMAND>

Commands:
  status     
  attach     
  focus      
  workspace  
  window     
  message    
  help       Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

##### `tt skill i3 status`

```text
Usage: status

Options:
  -h, --help
          Print help
```

##### `tt skill i3 attach`

```text
Usage: attach

Options:
  -h, --help
          Print help
```

##### `tt skill i3 focus`

```text
Usage: focus [OPTIONS]

Options:
      --workspace <WORKSPACE>
          

  -h, --help
          Print help
```

##### `tt skill i3 workspace`

```text
Usage: workspace <COMMAND>

Commands:
  focus  
  move   
  list   
  help   Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

###### `tt skill i3 workspace focus`

```text
Usage: focus --workspace <WORKSPACE>

Options:
      --workspace <WORKSPACE>
          

  -h, --help
          Print help
```

###### `tt skill i3 workspace move`

```text
Usage: move --workspace <WORKSPACE>

Options:
      --workspace <WORKSPACE>
          

  -h, --help
          Print help
```

###### `tt skill i3 workspace list`

```text
Usage: list

Options:
  -h, --help
          Print help
```

##### `tt skill i3 window`

```text
Usage: window <COMMAND>

Commands:
  focus  
  move   
  close  
  info   
  help   Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

###### `tt skill i3 window focus`

```text
Usage: focus --criteria <CRITERIA>

Options:
      --criteria <CRITERIA>
          

  -h, --help
          Print help
```

###### `tt skill i3 window move`

```text
Usage: move --criteria <CRITERIA> --workspace <WORKSPACE>

Options:
      --criteria <CRITERIA>
          

      --workspace <WORKSPACE>
          

  -h, --help
          Print help
```

###### `tt skill i3 window close`

```text
Usage: close --criteria <CRITERIA>

Options:
      --criteria <CRITERIA>
          

  -h, --help
          Print help
```

###### `tt skill i3 window info`

```text
Usage: info --criteria <CRITERIA>

Options:
      --criteria <CRITERIA>
          

  -h, --help
          Print help
```

##### `tt skill i3 message`

```text
Usage: message [MESSAGE]...

Arguments:
  [MESSAGE]...
          

Options:
  -h, --help
          Print help
```

#### `tt skill tt`

```text
Usage: tt <COMMAND>

Commands:
  status      
  spawn       
  resume      
  app-server  
  help        Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

##### `tt skill tt status`

```text
Usage: status

Options:
  -h, --help
          Print help
```

##### `tt skill tt spawn`

```text
Usage: spawn [OPTIONS] <ROLE>

Arguments:
  <ROLE>
          

Options:
      --workstream <WORKSTREAM>
          

      --new-workstream <NEW_WORKSTREAM>
          

      --repo-root <REPO_ROOT>
          

      --headless
          

      --model <MODEL>
          

  -h, --help
          Print help
```

##### `tt skill tt resume`

```text
Usage: resume [OPTIONS] <THREAD>

Arguments:
  <THREAD>
          

Options:
      --cwd <CWD>
          

      --model <MODEL>
          

  -h, --help
          Print help
```

##### `tt skill tt app-server`

```text
Usage: app-server <COMMAND>

Commands:
  status   
  start    
  stop     
  restart  
  help     Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

###### `tt skill tt app-server status`

```text
Usage: status [NAME]

Arguments:
  [NAME]
          [default: default]

Options:
  -h, --help
          Print help
```

###### `tt skill tt app-server start`

```text
Usage: start [NAME]

Arguments:
  [NAME]
          [default: default]

Options:
  -h, --help
          Print help
```

###### `tt skill tt app-server stop`

```text
Usage: stop [NAME]

Arguments:
  [NAME]
          [default: default]

Options:
  -h, --help
          Print help
```

###### `tt skill tt app-server restart`

```text
Usage: restart [NAME]

Arguments:
  [NAME]
          [default: default]

Options:
  -h, --help
          Print help
```

#### `tt skill process`

```text
Usage: process <COMMAND>

Commands:
  status   
  inspect  
  start    
  stop     
  restart  
  signal   
  tree     
  help     Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

##### `tt skill process status`

```text
Usage: status [OPTIONS]

Options:
      --pid <PID>
          

      --name <NAME>
          

  -h, --help
          Print help
```

##### `tt skill process inspect`

```text
Usage: inspect [OPTIONS]

Options:
      --pid <PID>
          

      --name <NAME>
          

  -h, --help
          Print help
```

##### `tt skill process start`

```text
Usage: start [OPTIONS] [COMMAND]...

Arguments:
  [COMMAND]...
          

Options:
      --pid <PID>
          

      --name <NAME>
          

      --cwd <CWD>
          

  -h, --help
          Print help
```

##### `tt skill process stop`

```text
Usage: stop [OPTIONS]

Options:
      --pid <PID>
          

      --name <NAME>
          

  -h, --help
          Print help
```

##### `tt skill process restart`

```text
Usage: restart [OPTIONS] [COMMAND]...

Arguments:
  [COMMAND]...
          

Options:
      --pid <PID>
          

      --name <NAME>
          

      --cwd <CWD>
          

  -h, --help
          Print help
```

##### `tt skill process signal`

```text
Usage: signal [OPTIONS]

Options:
      --pid <PID>
          

      --name <NAME>
          

      --signal <SIGNAL>
          [default: TERM]

  -h, --help
          Print help
```

##### `tt skill process tree`

```text
Usage: tree [OPTIONS]

Options:
      --pid <PID>
          

      --name <NAME>
          

  -h, --help
          Print help
```

#### `tt skill services`

```text
Usage: services <COMMAND>

Commands:
  status   
  inspect  
  start    
  stop     
  restart  
  reload   
  help     Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

##### `tt skill services status`

```text
Usage: status <SERVICE>

Arguments:
  <SERVICE>
          [possible values: daemon, app-server]

Options:
  -h, --help
          Print help
```

##### `tt skill services inspect`

```text
Usage: inspect <SERVICE>

Arguments:
  <SERVICE>
          [possible values: daemon, app-server]

Options:
  -h, --help
          Print help
```

##### `tt skill services start`

```text
Usage: start <SERVICE>

Arguments:
  <SERVICE>
          [possible values: daemon, app-server]

Options:
  -h, --help
          Print help
```

##### `tt skill services stop`

```text
Usage: stop <SERVICE>

Arguments:
  <SERVICE>
          [possible values: daemon, app-server]

Options:
  -h, --help
          Print help
```

##### `tt skill services restart`

```text
Usage: restart <SERVICE>

Arguments:
  <SERVICE>
          [possible values: daemon, app-server]

Options:
  -h, --help
          Print help
```

##### `tt skill services reload`

```text
Usage: reload <SERVICE>

Arguments:
  <SERVICE>
          [possible values: daemon, app-server]

Options:
  -h, --help
          Print help
```

#### `tt skill git`

```text
Usage: git <COMMAND>

Commands:
  status    
  branch    
  worktree  
  help      Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

##### `tt skill git status`

```text
Usage: status [OPTIONS]

Options:
      --repo-root <REPO_ROOT>
          

      --worktree-path <WORKTREE_PATH>
          

  -h, --help
          Print help
```

##### `tt skill git branch`

```text
Usage: branch <COMMAND>

Commands:
  current  
  list     
  help     Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

###### `tt skill git branch current`

```text
Usage: current [OPTIONS]

Options:
      --repo-root <REPO_ROOT>
          

      --worktree-path <WORKTREE_PATH>
          

  -h, --help
          Print help
```

###### `tt skill git branch list`

```text
Usage: list [OPTIONS]

Options:
      --repo-root <REPO_ROOT>
          

      --worktree-path <WORKTREE_PATH>
          

  -h, --help
          Print help
```

##### `tt skill git worktree`

```text
Usage: worktree <COMMAND>

Commands:
  current  
  list     
  help     Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

###### `tt skill git worktree current`

```text
Usage: current [OPTIONS]

Options:
      --repo-root <REPO_ROOT>
          

      --worktree-path <WORKTREE_PATH>
          

  -h, --help
          Print help
```

###### `tt skill git worktree list`

```text
Usage: list [OPTIONS]

Options:
      --repo-root <REPO_ROOT>
          

      --worktree-path <WORKTREE_PATH>
          

  -h, --help
          Print help
```

### `tt tt`

```text
Usage: tt <COMMAND>

Commands:
  models    
  spawn     
  resume    
  worktree  TT lane worktree lifecycle helpers
  threads   
  turns     
  help      Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

#### `tt tt models`

```text
Usage: models <COMMAND>

Commands:
  list  
  help  Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

##### `tt tt models list`

```text
Usage: list --workstream <WORKSTREAM>

Options:
      --workstream <WORKSTREAM>
          

  -h, --help
          Print help
```

#### `tt tt spawn`

```text
Usage: spawn [OPTIONS] <ROLE>

Arguments:
  <ROLE>
          

Options:
      --workstream <WORKSTREAM>
          

      --new-workstream <NEW_WORKSTREAM>
          

      --repo-root <REPO_ROOT>
          

      --headless
          

      --model <MODEL>
          

  -h, --help
          Print help
```

#### `tt tt resume`

```text
Usage: resume [OPTIONS] <THREAD>

Arguments:
  <THREAD>
          

Options:
      --cwd <CWD>
          

      --model <MODEL>
          

  -h, --help
          Print help
```

#### `tt tt worktree`

```text
TT lane worktree lifecycle helpers

Usage: worktree <COMMAND>

Commands:
  add    
  prune  
  help   Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

##### `tt tt worktree add`

```text
Usage: add <REPO_ROOT> <NAME>

Arguments:
  <REPO_ROOT>
          

  <NAME>
          

Options:
  -h, --help
          Print help
```

##### `tt tt worktree prune`

```text
Usage: prune <SELECTOR>

Arguments:
  <SELECTOR>
          

Options:
  -h, --help
          Print help
```

#### `tt tt threads`

```text
Usage: threads <COMMAND>

Commands:
  list         
  list-loaded  
  read         
  start        
  resume       
  help         Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

##### `tt tt threads list`

```text
Usage: list --workstream <WORKSTREAM>

Options:
      --workstream <WORKSTREAM>
          

  -h, --help
          Print help
```

##### `tt tt threads list-loaded`

```text
Usage: list-loaded --workstream <WORKSTREAM>

Options:
      --workstream <WORKSTREAM>
          

  -h, --help
          Print help
```

##### `tt tt threads read`

```text
Usage: read --thread <THREAD>

Options:
      --thread <THREAD>
          

  -h, --help
          Print help
```

##### `tt tt threads start`

```text
Usage: start [OPTIONS]

Options:
      --cwd <CWD>
          

      --model <MODEL>
          

      --ephemeral
          

  -h, --help
          Print help
```

##### `tt tt threads resume`

```text
Usage: resume [OPTIONS] --thread <THREAD>

Options:
      --thread <THREAD>
          

      --cwd <CWD>
          

      --model <MODEL>
          

  -h, --help
          Print help
```

#### `tt tt turns`

```text
Usage: turns <COMMAND>

Commands:
  list-active  
  recent       
  get          
  help         Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

##### `tt tt turns list-active`

```text
Usage: list-active

Options:
  -h, --help
          Print help
```

##### `tt tt turns recent`

```text
Usage: recent [OPTIONS] --thread <THREAD>

Options:
      --thread <THREAD>
          

      --limit <LIMIT>
          [default: 10]

  -h, --help
          Print help
```

##### `tt tt turns get`

```text
Usage: get --thread <THREAD> --turn <TURN>

Options:
      --thread <THREAD>
          

      --turn <TURN>
          

  -h, --help
          Print help
```

### `tt prompt`

```text
Usage: prompt --thread <THREAD> --text <TEXT>

Options:
      --thread <THREAD>
          

      --text <TEXT>
          

  -h, --help
          Print help
```

### `tt quickstart`

```text
Usage: quickstart [OPTIONS] --text <TEXT>

Options:
      --cwd <CWD>
          

      --model <MODEL>
          

      --text <TEXT>
          

  -h, --help
          Print help
```

