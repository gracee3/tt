# TT CLI Reference

Generated from the live `tt` Clap tree. Regenerate with `tt docs export-cli --out docs/CLI_REFERENCE.md`.

## `tt`

```text
tt control plane

Usage: tt [OPTIONS] <COMMAND>

Commands:
  daemon      Launch and manage the tt daemon
  doctor      Inspect the current TT state and surfaces
  docs        Export rendered CLI documentation
  remote      Run TT commands against a remote runtime
  events      Inspect the recent TT event stream
  project     Manage durable TT project records
  worktree    Canonical authority-backed CRUD for planning work units
  todo        Capture notes, review gaps, and turn TODOs into plans
  develop     Start an implementation thread for the current branch
  test        Start a validation thread for the current branch
  integrate   Start a repo-branch coordination thread
  chat        Start a discuss-only thread
  learn       Start a recon and gap-finding thread
  handoff     Start a handoff thread
  diff        Inspect tracked and untracked changes before cleanup
  split       Fork a new child thread and worktree from the current context
  close       Tear down the current worktree according to policy
  park        Suspend the current worktree without cleanup
  worktrees   Inspect and manage TT-derived git worktrees
  app-server  Manage the shared tt app-server lifecycle
  lane        Manage lane-local runtimes and rendered directory state
  snapshot    Create, fork, diff, and prune TT snapshots
  context     Edit snapshot context selection and pinning
  workspace   Bind snapshots to workspace and git state
  tui         Open the tt dashboard TUI
  app         Invoke the TT app-embedded command surface
  i3          Coordinate the desktop window manager
  skill       Run a typed skill runtime command
  prompt      Send a single prompt to a thread
  quickstart  Launch a quick TT session from freeform input
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
Inspect the current TT state and surfaces

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
Run TT commands against a remote runtime

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
Inspect the recent TT event stream

Usage: events <COMMAND>

Commands:
  recent  Show recent events
  watch   Watch events in real time
  help    Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

#### `tt events recent`

```text
Show recent events

Usage: recent [OPTIONS]

Options:
      --limit <LIMIT>
          Maximum number of events to return
          
          [default: 20]

  -h, --help
          Print help
```

#### `tt events watch`

```text
Watch events in real time

Usage: watch [OPTIONS]

Options:
      --snapshot
          Include full snapshot data in the event stream

      --count <COUNT>
          Maximum number of events to watch before stopping

  -h, --help
          Print help
```

### `tt project`

```text
Manage durable TT project records

Usage: project <COMMAND>

Commands:
  add     Add a project record to a repository
  create  Create a durable project record
  edit    Edit a durable project record
  delete  Delete a durable project record
  list    List durable project records
  get     Get a durable project record
  help    Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

#### `tt project add`

```text
Add a project record to a repository

Usage: add <REPO_ROOT> <NAME>

Arguments:
  <REPO_ROOT>
          Repository root to add the project record under

  <NAME>
          Project name to add

Options:
  -h, --help
          Print help
```

#### `tt project create`

```text
Create a durable project record

Usage: create [OPTIONS] --title <TITLE> --objective <OBJECTIVE>

Options:
      --title <TITLE>
          Title for the durable project record

      --objective <OBJECTIVE>
          Objective for the durable project record

      --priority <PRIORITY>
          Optional priority label

      --tt-home <TT_HOME>
          Optional TT home directory override

      --sqlite-home <SQLITE_HOME>
          Optional SQLite home directory override

      --listen-url <LISTEN_URL>
          Optional listen URL override

      --transport-kind <TRANSPORT_KIND>
          Transport used to connect the workstream
          
          [possible values: local-app-server, remote-websocket]

      --app-server-policy <APP_SERVER_POLICY>
          How app-server instances are managed
          
          [possible values: shared-current-daemon, dedicated-per-workstream]

      --connection-mode <CONNECTION_MODE>
          How execution connects to the workstream
          
          [possible values: connect-only, spawn-if-needed, spawn-always]

  -h, --help
          Print help
```

#### `tt project edit`

```text
Edit a durable project record

Usage: edit [OPTIONS] --workstream <WORKSTREAM>

Options:
      --workstream <WORKSTREAM>
          Project record id or slug to edit

      --title <TITLE>
          Updated title

      --objective <OBJECTIVE>
          Updated objective

      --status <STATUS>
          Updated workstream status
          
          [possible values: active, blocked, completed]

      --priority <PRIORITY>
          Updated priority label

      --tt-home <TT_HOME>
          Updated TT home directory override

      --sqlite-home <SQLITE_HOME>
          Updated SQLite home directory override

      --listen-url <LISTEN_URL>
          Updated listen URL override

      --transport-kind <TRANSPORT_KIND>
          Updated transport kind
          
          [possible values: local-app-server, remote-websocket]

      --app-server-policy <APP_SERVER_POLICY>
          Updated app-server policy
          
          [possible values: shared-current-daemon, dedicated-per-workstream]

      --connection-mode <CONNECTION_MODE>
          Updated execution connection mode
          
          [possible values: connect-only, spawn-if-needed, spawn-always]

      --clear-execution-scope
          Clear any execution-scope override

  -h, --help
          Print help
```

#### `tt project delete`

```text
Delete a durable project record

Usage: delete <WORKSTREAM>

Arguments:
  <WORKSTREAM>
          Project record id or slug to delete

Options:
  -h, --help
          Print help
```

#### `tt project list`

```text
List durable project records

Usage: list

Options:
  -h, --help
          Print help
```

#### `tt project get`

```text
Get a durable project record

Usage: get --workstream <WORKSTREAM>

Options:
      --workstream <WORKSTREAM>
          Project record id or slug to inspect

  -h, --help
          Print help
```

### `tt worktree`

```text
Canonical authority-backed CRUD for planning work units

Usage: worktree <COMMAND>

Commands:
  create     Create a planning work unit
  edit       Edit a planning work unit
  delete     Delete a planning work unit
  list       List planning work units
  get        Get a planning work unit
  thread     Work with tracked-thread planning records
  workspace  Work with tracked-thread planning record workspaces
  help       Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

#### `tt worktree create`

```text
Create a planning work unit

Usage: create [OPTIONS] --workstream <WORKSTREAM> --title <TITLE> --task <TASK>

Options:
      --workstream <WORKSTREAM>
          Parent workstream id or slug

      --title <TITLE>
          Work unit title

      --task <TASK>
          Work unit task description

      --dependency <DEPENDENCIES>
          Dependent work unit ids

  -h, --help
          Print help
```

#### `tt worktree edit`

```text
Edit a planning work unit

Usage: edit [OPTIONS] --workunit <WORKUNIT>

Options:
      --workunit <WORKUNIT>
          Work unit id or slug to edit

      --title <TITLE>
          Updated title

      --task <TASK>
          Updated task description

      --status <STATUS>
          Updated work unit status
          
          [possible values: ready, blocked, running, awaiting-decision, accepted, needs-human, completed]

  -h, --help
          Print help
```

#### `tt worktree delete`

```text
Delete a planning work unit

Usage: delete --workunit <WORKUNIT>

Options:
      --workunit <WORKUNIT>
          Work unit id or slug to inspect

  -h, --help
          Print help
```

#### `tt worktree list`

```text
List planning work units

Usage: list [OPTIONS]

Options:
      --workstream <WORKSTREAM>
          Optional workstream filter

  -h, --help
          Print help
```

#### `tt worktree get`

```text
Get a planning work unit

Usage: get --workunit <WORKUNIT>

Options:
      --workunit <WORKUNIT>
          Work unit id or slug to inspect

  -h, --help
          Print help
```

#### `tt worktree thread`

```text
Work with tracked-thread planning records

Usage: thread <COMMAND>

Commands:
  add     Add a tracked thread to a work unit
  set     Update a tracked thread
  remove  Remove a tracked thread from a work unit
  list    List tracked threads for a work unit
  get     Get a tracked thread record
  help    Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

##### `tt worktree thread add`

```text
Add a tracked thread to a work unit

Usage: add [OPTIONS] --workunit <WORKUNIT> --title <TITLE> --root-dir <ROOT_DIR>

Options:
      --workunit <WORKUNIT>
          Parent work unit id

      --title <TITLE>
          Tracked-thread title

      --root-dir <ROOT_DIR>
          Root directory for the tracked thread

      --notes <NOTES>
          Optional thread notes

      --upstream-thread <UPSTREAM_THREAD>
          Optional upstream thread id

      --model <MODEL>
          Optional model override

      --workspace-repository-root <REPOSITORY_ROOT>
          Workspace repository root

      --workspace-worktree-path <WORKTREE_PATH>
          Workspace worktree path

      --workspace-branch-name <BRANCH_NAME>
          Workspace branch name

      --workspace-base-ref <BASE_REF>
          Workspace base ref

      --workspace-base-commit <BASE_COMMIT>
          Workspace base commit

      --workspace-landing-target <LANDING_TARGET>
          Workspace landing target

      --workspace-strategy <STRATEGY>
          Workspace strategy
          
          [possible values: shared, dedicated-thread-worktree, ephemeral]

      --workspace-landing-policy <LANDING_POLICY>
          Workspace landing policy
          
          [possible values: merge-to-main, merge-to-campaign, cherry-pick-only, parked]

      --workspace-sync-policy <SYNC_POLICY>
          Workspace sync policy
          
          [possible values: manual, rebase-before-completion, rebase-before-each-assignment]

      --workspace-cleanup-policy <CLEANUP_POLICY>
          Workspace cleanup policy
          
          [possible values: keep-until-campaign-closed, prune-after-merge, keep-for-audit]

      --workspace-status <STATUS>
          Workspace status
          
          [possible values: requested, ready, dirty, ahead, behind, conflicted, merged, abandoned, pruned]

      --workspace-last-reported-head-commit <LAST_REPORTED_HEAD_COMMIT>
          Last head commit reported for the workspace

  -h, --help
          Print help
```

##### `tt worktree thread set`

```text
Update a tracked thread

Usage: set [OPTIONS] --tracked-thread <TRACKED_THREAD>

Options:
      --tracked-thread <TRACKED_THREAD>
          Tracked-thread id to edit

      --title <TITLE>
          Updated title

      --root-dir <ROOT_DIR>
          Updated root directory

      --notes <NOTES>
          Updated thread notes

      --upstream-thread <UPSTREAM_THREAD>
          Updated upstream thread id

      --binding-state <BINDING_STATE>
          Updated binding state
          
          [possible values: unbound, bound, detached, missing]

      --model <MODEL>
          Optional model override

      --workspace-repository-root <REPOSITORY_ROOT>
          Workspace repository root

      --workspace-worktree-path <WORKTREE_PATH>
          Workspace worktree path

      --workspace-branch-name <BRANCH_NAME>
          Workspace branch name

      --workspace-base-ref <BASE_REF>
          Workspace base ref

      --workspace-base-commit <BASE_COMMIT>
          Workspace base commit

      --workspace-landing-target <LANDING_TARGET>
          Workspace landing target

      --workspace-strategy <STRATEGY>
          Workspace strategy
          
          [possible values: shared, dedicated-thread-worktree, ephemeral]

      --workspace-landing-policy <LANDING_POLICY>
          Workspace landing policy
          
          [possible values: merge-to-main, merge-to-campaign, cherry-pick-only, parked]

      --workspace-sync-policy <SYNC_POLICY>
          Workspace sync policy
          
          [possible values: manual, rebase-before-completion, rebase-before-each-assignment]

      --workspace-cleanup-policy <CLEANUP_POLICY>
          Workspace cleanup policy
          
          [possible values: keep-until-campaign-closed, prune-after-merge, keep-for-audit]

      --workspace-status <STATUS>
          Workspace status
          
          [possible values: requested, ready, dirty, ahead, behind, conflicted, merged, abandoned, pruned]

      --workspace-last-reported-head-commit <LAST_REPORTED_HEAD_COMMIT>
          Last head commit reported for the workspace

  -h, --help
          Print help
```

##### `tt worktree thread remove`

```text
Remove a tracked thread from a work unit

Usage: remove [OPTIONS] --tracked-thread <TRACKED_THREAD>

Options:
      --tracked-thread <TRACKED_THREAD>
          Tracked-thread id to inspect

      --request-note <REQUEST_NOTE>
          Optional request note for the operation

  -h, --help
          Print help
```

##### `tt worktree thread list`

```text
List tracked threads for a work unit

Usage: list --workunit <WORKUNIT>

Options:
      --workunit <WORKUNIT>
          Work unit id to list tracked threads for

  -h, --help
          Print help
```

##### `tt worktree thread get`

```text
Get a tracked thread record

Usage: get [OPTIONS] --tracked-thread <TRACKED_THREAD>

Options:
      --tracked-thread <TRACKED_THREAD>
          Tracked-thread id to inspect

      --request-note <REQUEST_NOTE>
          Optional request note for the operation

  -h, --help
          Print help
```

#### `tt worktree workspace`

```text
Work with tracked-thread planning record workspaces

Usage: workspace <COMMAND>

Commands:
  prepare-workspace  Prepare the tracked-thread workspace
  refresh-workspace  Refresh the tracked-thread workspace
  merge-prep         Assess merge readiness for the workspace
  authorize-merge    Authorize merging the workspace
  execute-landing    Execute landing for the workspace
  prune-workspace    Prune the tracked-thread workspace
  help               Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

##### `tt worktree workspace prepare-workspace`

```text
Prepare the tracked-thread workspace

Usage: prepare-workspace [OPTIONS] --tracked-thread <TRACKED_THREAD>

Options:
      --tracked-thread <TRACKED_THREAD>
          Tracked-thread id to inspect

      --request-note <REQUEST_NOTE>
          Optional request note for the operation

  -h, --help
          Print help
```

##### `tt worktree workspace refresh-workspace`

```text
Refresh the tracked-thread workspace

Usage: refresh-workspace [OPTIONS] --tracked-thread <TRACKED_THREAD>

Options:
      --tracked-thread <TRACKED_THREAD>
          Tracked-thread id to inspect

      --request-note <REQUEST_NOTE>
          Optional request note for the operation

  -h, --help
          Print help
```

##### `tt worktree workspace merge-prep`

```text
Assess merge readiness for the workspace

Usage: merge-prep [OPTIONS] --tracked-thread <TRACKED_THREAD>

Options:
      --tracked-thread <TRACKED_THREAD>
          Tracked-thread id to inspect

      --request-note <REQUEST_NOTE>
          Optional request note for the operation

  -h, --help
          Print help
```

##### `tt worktree workspace authorize-merge`

```text
Authorize merging the workspace

Usage: authorize-merge [OPTIONS] --tracked-thread <TRACKED_THREAD>

Options:
      --tracked-thread <TRACKED_THREAD>
          Tracked-thread id to inspect

      --request-note <REQUEST_NOTE>
          Optional request note for the operation

  -h, --help
          Print help
```

##### `tt worktree workspace execute-landing`

```text
Execute landing for the workspace

Usage: execute-landing [OPTIONS] --tracked-thread <TRACKED_THREAD>

Options:
      --tracked-thread <TRACKED_THREAD>
          Tracked-thread id to inspect

      --request-note <REQUEST_NOTE>
          Optional request note for the operation

  -h, --help
          Print help
```

##### `tt worktree workspace prune-workspace`

```text
Prune the tracked-thread workspace

Usage: prune-workspace [OPTIONS] --tracked-thread <TRACKED_THREAD>

Options:
      --tracked-thread <TRACKED_THREAD>
          Tracked-thread id to inspect

      --request-note <REQUEST_NOTE>
          Optional request note for the operation

  -h, --help
          Print help
```

### `tt todo`

```text
Capture notes, review gaps, and turn TODOs into plans

Usage: todo <COMMAND>

Commands:
  note    Ingest notes into the active TODO ledger
  review  Ask clarifying questions about the active TODO section
  plan    Turn the active TODO section into a plan
  help    Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

#### `tt todo note`

```text
Ingest notes into the active TODO ledger

Usage: note [OPTIONS]

Options:
      --workstream <WORKSTREAM>
          Existing workstream to attach the mode thread to

      --new-workstream <NEW_WORKSTREAM>
          Create a new workstream for the mode thread

      --repo-root <REPO_ROOT>
          Repository root to bind the mode thread to

      --headless
          Spawn the thread without a visible UI

      --model <MODEL>
          Model to use for the spawned thread

  -h, --help
          Print help
```

#### `tt todo review`

```text
Ask clarifying questions about the active TODO section

Usage: review [OPTIONS]

Options:
      --workstream <WORKSTREAM>
          Existing workstream to attach the mode thread to

      --new-workstream <NEW_WORKSTREAM>
          Create a new workstream for the mode thread

      --repo-root <REPO_ROOT>
          Repository root to bind the mode thread to

      --headless
          Spawn the thread without a visible UI

      --model <MODEL>
          Model to use for the spawned thread

  -h, --help
          Print help
```

#### `tt todo plan`

```text
Turn the active TODO section into a plan

Usage: plan [OPTIONS]

Options:
      --workstream <WORKSTREAM>
          Existing workstream to attach the mode thread to

      --new-workstream <NEW_WORKSTREAM>
          Create a new workstream for the mode thread

      --repo-root <REPO_ROOT>
          Repository root to bind the mode thread to

      --headless
          Spawn the thread without a visible UI

      --model <MODEL>
          Model to use for the spawned thread

  -h, --help
          Print help
```

### `tt develop`

```text
Start an implementation thread for the current branch

Usage: develop [OPTIONS]

Options:
      --workstream <WORKSTREAM>
          Existing workstream to attach the mode thread to

      --new-workstream <NEW_WORKSTREAM>
          Create a new workstream for the mode thread

      --repo-root <REPO_ROOT>
          Repository root to bind the mode thread to

      --headless
          Spawn the thread without a visible UI

      --model <MODEL>
          Model to use for the spawned thread

  -h, --help
          Print help
```

### `tt test`

```text
Start a validation thread for the current branch

Usage: test [OPTIONS]

Options:
      --workstream <WORKSTREAM>
          Existing workstream to attach the mode thread to

      --new-workstream <NEW_WORKSTREAM>
          Create a new workstream for the mode thread

      --repo-root <REPO_ROOT>
          Repository root to bind the mode thread to

      --headless
          Spawn the thread without a visible UI

      --model <MODEL>
          Model to use for the spawned thread

  -h, --help
          Print help
```

### `tt integrate`

```text
Start a repo-branch coordination thread

Usage: integrate [OPTIONS]

Options:
      --workstream <WORKSTREAM>
          Existing workstream to attach the mode thread to

      --new-workstream <NEW_WORKSTREAM>
          Create a new workstream for the mode thread

      --repo-root <REPO_ROOT>
          Repository root to bind the mode thread to

      --headless
          Spawn the thread without a visible UI

      --model <MODEL>
          Model to use for the spawned thread

  -h, --help
          Print help
```

### `tt chat`

```text
Start a discuss-only thread

Usage: chat [OPTIONS]

Options:
      --workstream <WORKSTREAM>
          Existing workstream to attach the mode thread to

      --new-workstream <NEW_WORKSTREAM>
          Create a new workstream for the mode thread

      --repo-root <REPO_ROOT>
          Repository root to bind the mode thread to

      --headless
          Spawn the thread without a visible UI

      --model <MODEL>
          Model to use for the spawned thread

  -h, --help
          Print help
```

### `tt learn`

```text
Start a recon and gap-finding thread

Usage: learn [OPTIONS]

Options:
      --workstream <WORKSTREAM>
          Existing workstream to attach the mode thread to

      --new-workstream <NEW_WORKSTREAM>
          Create a new workstream for the mode thread

      --repo-root <REPO_ROOT>
          Repository root to bind the mode thread to

      --headless
          Spawn the thread without a visible UI

      --model <MODEL>
          Model to use for the spawned thread

  -h, --help
          Print help
```

### `tt handoff`

```text
Start a handoff thread

Usage: handoff [OPTIONS]

Options:
      --workstream <WORKSTREAM>
          Existing workstream to attach the mode thread to

      --new-workstream <NEW_WORKSTREAM>
          Create a new workstream for the mode thread

      --repo-root <REPO_ROOT>
          Repository root to bind the mode thread to

      --headless
          Spawn the thread without a visible UI

      --model <MODEL>
          Model to use for the spawned thread

  -h, --help
          Print help
```

### `tt diff`

```text
Inspect tracked and untracked changes before cleanup

Usage: diff [OPTIONS]

Options:
      --selector <SELECTOR>
          Optional selector for the branch or worktree to inspect

      --repo-root <REPO_ROOT>
          Optional repository root to inspect

      --worktree-path <WORKTREE_PATH>
          Optional worktree path to inspect

  -h, --help
          Print help
```

### `tt split`

```text
Fork a new child thread and worktree from the current context

Usage: split [OPTIONS]

Options:
      --role <ROLE>
          Override the role for the child thread

      --workstream <WORKSTREAM>
          Existing workstream to attach the mode thread to

      --new-workstream <NEW_WORKSTREAM>
          Create a new workstream for the mode thread

      --repo-root <REPO_ROOT>
          Repository root to bind the mode thread to

      --headless
          Spawn the thread without a visible UI

      --model <MODEL>
          Model to use for the spawned thread

      --ephemeral
          Mark the split thread as ephemeral

  -h, --help
          Print help
```

### `tt close`

```text
Tear down the current worktree according to policy

Usage: close [OPTIONS] <SELECTOR>

Arguments:
  <SELECTOR>
          Selector describing the thread, branch, or workspace to close

Options:
      --force
          Force close even when safety checks fail

  -h, --help
          Print help
```

### `tt park`

```text
Suspend the current worktree without cleanup

Usage: park [OPTIONS] <SELECTOR>

Arguments:
  <SELECTOR>
          Selector describing the thread, branch, or workspace to park

Options:
      --note <NOTE>
          Optional note to carry with the parked state

  -h, --help
          Print help
```

### `tt worktrees`

```text
Inspect and manage TT-derived git worktrees

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
  add      Register a named app-server instance
  remove   Forget a named app-server instance
  start    Start a named app-server instance
  stop     Stop a named app-server instance
  restart  Restart a named app-server instance
  status   Show the status of a named app-server instance
  info     Show the configuration of a named app-server instance
  help     Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

#### `tt app-server add`

```text
Register a named app-server instance

Usage: add [NAME]

Arguments:
  [NAME]
          Named app-server instance to target
          
          [default: default]

Options:
  -h, --help
          Print help
```

#### `tt app-server remove`

```text
Forget a named app-server instance

Usage: remove [NAME]

Arguments:
  [NAME]
          Named app-server instance to target
          
          [default: default]

Options:
  -h, --help
          Print help
```

#### `tt app-server start`

```text
Start a named app-server instance

Usage: start [NAME]

Arguments:
  [NAME]
          Named app-server instance to target
          
          [default: default]

Options:
  -h, --help
          Print help
```

#### `tt app-server stop`

```text
Stop a named app-server instance

Usage: stop [NAME]

Arguments:
  [NAME]
          Named app-server instance to target
          
          [default: default]

Options:
  -h, --help
          Print help
```

#### `tt app-server restart`

```text
Restart a named app-server instance

Usage: restart [NAME]

Arguments:
  [NAME]
          Named app-server instance to target
          
          [default: default]

Options:
  -h, --help
          Print help
```

#### `tt app-server status`

```text
Show the status of a named app-server instance

Usage: status [NAME]

Arguments:
  [NAME]
          Named app-server instance to target
          
          [default: default]

Options:
  -h, --help
          Print help
```

#### `tt app-server info`

```text
Show the configuration of a named app-server instance

Usage: info [NAME]

Arguments:
  [NAME]
          Named app-server instance to target
          
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
  init     Bootstrap a new lane with rendered directory state and repo checkouts
  inspect  Print the current lane manifest, worktrees, and attachment summary
  attach   Bind a tracked thread to a lane workspace
  detach   Unbind a tracked thread from a lane workspace
  cleanup  Clean up lane runtime state according to the requested scope
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
Bootstrap a new lane with rendered directory state and repo checkouts

Usage: init [OPTIONS] <LABEL>

Arguments:
  <LABEL>
          Human-readable lane label to normalize into the lane slug

Options:
      --repo <REPOS>
          Repo to include in the lane in org/repo form; repeat for multiple repos

  -h, --help
          Print help
```

#### `tt lane inspect`

```text
Print the current lane manifest, worktrees, and attachment summary

Usage: inspect <LABEL>

Arguments:
  <LABEL>
          Human-readable lane label to inspect

Options:
  -h, --help
          Print help
```

#### `tt lane attach`

```text
Bind a tracked thread to a lane workspace

Usage: attach [OPTIONS] --repo <REPO> --tracked-thread <TRACKED_THREAD> <LABEL>

Arguments:
  <LABEL>
          Human-readable lane label that owns the workspace

Options:
      --repo <REPO>
          Repo to bind in org/repo form

      --workspace <WORKSPACE>
          Workspace name within the lane repo; defaults to `default`

      --tracked-thread <TRACKED_THREAD>
          Authority tracked-thread id to attach to the lane workspace

  -h, --help
          Print help
```

#### `tt lane detach`

```text
Unbind a tracked thread from a lane workspace

Usage: detach [OPTIONS] --repo <REPO> --tracked-thread <TRACKED_THREAD> <LABEL>

Arguments:
  <LABEL>
          Human-readable lane label that owns the workspace

Options:
      --repo <REPO>
          Repo to unbind in org/repo form

      --workspace <WORKSPACE>
          Workspace name within the lane repo; defaults to `default`

      --tracked-thread <TRACKED_THREAD>
          Authority tracked-thread id to detach from the lane workspace

  -h, --help
          Print help
```

#### `tt lane cleanup`

```text
Clean up lane runtime state according to the requested scope

Usage: cleanup [OPTIONS] <LABEL>

Arguments:
  <LABEL>
          Human-readable lane label to clean up

Options:
      --repo <REPO>
          Optional repo scope in org/repo form

      --workspace <WORKSPACE>
          Optional workspace name within the lane repo

      --scope <SCOPE>
          Cleanup scope to apply: runtime, worktree, repo, or lane

          Possible values:
          - runtime:  Remove only runtime state
          - worktree: Remove runtime state and the active worktree
          - repo:     Remove runtime state, the worktree, and the repo checkout
          - lane:     Remove the full lane subtree
          
          [default: runtime]

      --force
          Bypass safety checks for dirty or attached state

  -h, --help
          Print help (see a summary with '-h')
```

### `tt snapshot`

```text
Create, fork, diff, and prune TT snapshots

Usage: snapshot <COMMAND>

Commands:
  create   Create a snapshot
  fork     Fork a snapshot
  restore  Restore a snapshot
  diff     Diff two snapshots
  prune    Prune snapshots
  compact  Compact a noisy span into a summary snapshot
  list     List snapshots
  get      Get a snapshot
  help     Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

#### `tt snapshot create`

```text
Create a snapshot

Usage: create [OPTIONS] --lane <LANE> --repo <REPO> --workspace <WORKSPACE> --thread <THREAD>

Options:
      --lane <LANE>
          Lane label to scope the snapshot operation

      --repo <REPO>
          Repository to scope the snapshot operation

      --workspace <WORKSPACE>
          Workspace to scope the snapshot operation

      --thread <THREAD>
          Thread id to capture

      --include-turn-range <INCLUDE_TURN_RANGE>
          Turn range to include

      --exclude-turn-range <EXCLUDE_TURN_RANGE>
          Turn range to exclude

      --include-turn <INCLUDE_TURN>
          Turn id to include

      --exclude-turn <EXCLUDE_TURN>
          Turn id to exclude

      --pin-turn <PIN_TURN>
          Turn id to pin

      --pin-fact <PIN_FACT>
          Fact to pin into the snapshot

      --summary <SUMMARY>
          Snapshot summary

      --skill <SKILLS>
          Skill id to include

      --tag <TAGS>
          Tag to attach to the snapshot

      --created-by <CREATED_BY>
          Who created the snapshot

      --note <NOTE>
          Optional note for the snapshot

      --cwd <CWD>
          Optional cwd used to capture the snapshot

      --worktree <WORKTREE>
          Optional worktree path used for capture

      --commit <COMMIT>
          Optional commit id used for capture

      --branch <BRANCH>
          Optional branch name used for capture

      --model <MODEL>
          Optional model used for capture

  -h, --help
          Print help
```

#### `tt snapshot fork`

```text
Fork a snapshot

Usage: fork [OPTIONS] --from <FROM_SNAPSHOT>

Options:
      --from <FROM_SNAPSHOT>
          Snapshot id to fork from

      --created-by <CREATED_BY>
          Who created the fork

      --tag <TAGS>
          Tag to attach to the fork

      --note <NOTE>
          Optional fork note

  -h, --help
          Print help
```

#### `tt snapshot restore`

```text
Restore a snapshot

Usage: restore [OPTIONS] --snapshot <SNAPSHOT_ID>

Options:
      --snapshot <SNAPSHOT_ID>
          Snapshot id to restore

      --bind
          Bind the restored snapshot to the workspace

      --out <OUT>
          Optional output path for the restored artifact

  -h, --help
          Print help
```

#### `tt snapshot diff`

```text
Diff two snapshots

Usage: diff --from <FROM_SNAPSHOT> --to <TO_SNAPSHOT>

Options:
      --from <FROM_SNAPSHOT>
          Snapshot id to diff from

      --to <TO_SNAPSHOT>
          Snapshot id to diff to

  -h, --help
          Print help
```

#### `tt snapshot prune`

```text
Prune snapshots

Usage: prune [OPTIONS]

Options:
      --snapshot <SNAPSHOTS>
          Snapshot ids to prune

      --force
          Force prune even when references remain

  -h, --help
          Print help
```

#### `tt snapshot compact`

```text
Compact a noisy span into a summary snapshot

Usage: compact [OPTIONS] --from <FROM_SNAPSHOT> --summary <SUMMARY>

Options:
      --from <FROM_SNAPSHOT>
          Source snapshot id

      --summary <SUMMARY>
          Summary text to record

      --source-turn <SOURCE_TURN>
          Turn id used as summary source

      --created-by <CREATED_BY>
          Who created the derived snapshot

      --tag <TAGS>
          Tag to attach to the new snapshot

  -h, --help
          Print help
```

#### `tt snapshot list`

```text
List snapshots

Usage: list [OPTIONS]

Options:
      --lane <LANE>
          Optional lane filter

      --repo <REPO>
          Optional repository filter

      --workspace <WORKSPACE>
          Optional workspace filter

  -h, --help
          Print help
```

#### `tt snapshot get`

```text
Get a snapshot

Usage: get --snapshot <SNAPSHOT_ID>

Options:
      --snapshot <SNAPSHOT_ID>
          Snapshot id to inspect

  -h, --help
          Print help
```

### `tt context`

```text
Edit snapshot context selection and pinning

Usage: context <COMMAND>

Commands:
  include    Include turns or ranges into the next snapshot
  exclude    Exclude turns or ranges from the next snapshot
  pin        Pin facts and turns into the next snapshot
  summarize  Summarize a span into a derived snapshot
  help       Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

#### `tt context include`

```text
Include turns or ranges into the next snapshot

Usage: include [OPTIONS] --from <FROM_SNAPSHOT>

Options:
      --from <FROM_SNAPSHOT>
          Source snapshot id

      --include-turn-range <INCLUDE_TURN_RANGE>
          Turn range to include

      --exclude-turn-range <EXCLUDE_TURN_RANGE>
          Turn range to exclude

      --include-turn <INCLUDE_TURN>
          Turn id to include

      --exclude-turn <EXCLUDE_TURN>
          Turn id to exclude

      --pin-turn <PIN_TURN>
          Turn id to pin

      --pin-fact <PIN_FACT>
          Fact to pin into the snapshot

      --summary <SUMMARY>
          Summary text to attach to the new snapshot

      --tag <TAGS>
          Tag to attach to the new snapshot

      --created-by <CREATED_BY>
          Who created the derived snapshot

  -h, --help
          Print help
```

#### `tt context exclude`

```text
Exclude turns or ranges from the next snapshot

Usage: exclude [OPTIONS] --from <FROM_SNAPSHOT>

Options:
      --from <FROM_SNAPSHOT>
          Source snapshot id

      --include-turn-range <INCLUDE_TURN_RANGE>
          Turn range to include

      --exclude-turn-range <EXCLUDE_TURN_RANGE>
          Turn range to exclude

      --include-turn <INCLUDE_TURN>
          Turn id to include

      --exclude-turn <EXCLUDE_TURN>
          Turn id to exclude

      --pin-turn <PIN_TURN>
          Turn id to pin

      --pin-fact <PIN_FACT>
          Fact to pin into the snapshot

      --summary <SUMMARY>
          Summary text to attach to the new snapshot

      --tag <TAGS>
          Tag to attach to the new snapshot

      --created-by <CREATED_BY>
          Who created the derived snapshot

  -h, --help
          Print help
```

#### `tt context pin`

```text
Pin facts and turns into the next snapshot

Usage: pin [OPTIONS] --from <FROM_SNAPSHOT>

Options:
      --from <FROM_SNAPSHOT>
          Source snapshot id

      --pin-turn <PIN_TURN>
          Turn id to pin

      --pin-fact <PIN_FACT>
          Fact to pin

      --created-by <CREATED_BY>
          Who created the derived snapshot

      --tag <TAGS>
          Tag to attach to the new snapshot

  -h, --help
          Print help
```

#### `tt context summarize`

```text
Summarize a span into a derived snapshot

Usage: summarize [OPTIONS] --from <FROM_SNAPSHOT> --summary <SUMMARY>

Options:
      --from <FROM_SNAPSHOT>
          Source snapshot id

      --summary <SUMMARY>
          Summary text to record

      --source-turn <SOURCE_TURN>
          Turn id used as summary source

      --created-by <CREATED_BY>
          Who created the derived snapshot

      --tag <TAGS>
          Tag to attach to the new snapshot

  -h, --help
          Print help
```

### `tt workspace`

```text
Bind snapshots to workspace and git state

Usage: workspace <COMMAND>

Commands:
  bind     Bind snapshot state to a workspace
  promote  Promote a workspace binding to canonical state
  help     Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

#### `tt workspace bind`

```text
Bind snapshot state to a workspace

Usage: bind [OPTIONS] --lane <LANE> --repo <REPO> --workspace <WORKSPACE>

Options:
      --lane <LANE>
          Lane label to scope the snapshot operation

      --repo <REPO>
          Repository to scope the snapshot operation

      --workspace <WORKSPACE>
          Workspace to scope the snapshot operation

      --snapshot <SNAPSHOT_ID>
          Snapshot id to bind

      --commit <COMMIT>
          Commit id to bind

      --worktree <WORKTREE>
          Worktree path to bind

      --branch <BRANCH>
          Branch name to bind

      --thread <THREAD>
          Thread id to bind

      --canonical
          Mark the binding as canonical

  -h, --help
          Print help
```

#### `tt workspace promote`

```text
Promote a workspace binding to canonical state

Usage: promote [OPTIONS] --lane <LANE> --repo <REPO> --workspace <WORKSPACE> --snapshot <SNAPSHOT_ID>

Options:
      --lane <LANE>
          Lane label to scope the snapshot operation

      --repo <REPO>
          Repository to scope the snapshot operation

      --workspace <WORKSPACE>
          Workspace to scope the snapshot operation

      --snapshot <SNAPSHOT_ID>
          Snapshot id to promote

      --commit <COMMIT>
          Commit id to promote

      --worktree <WORKTREE>
          Worktree path to promote

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

### `tt app`

```text
Invoke the TT app-embedded command surface

Usage: app <COMMAND>

Commands:
  tt    Invoke the embedded TT command surface
  help  Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

#### `tt app tt`

```text
Invoke the embedded TT command surface

Usage: tt <COMMAND>

Commands:
  models    Inspect available TT models
  spawn     Spawn a role-backed TT thread
  resume    Resume a TT thread
  worktree  Manage TT worktrees
  threads   Manage TT thread records
  turns     Inspect TT turns
  help      Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

##### `tt app tt models`

```text
Inspect available TT models

Usage: models <COMMAND>

Commands:
  list  List models for a workstream
  help  Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

###### `tt app tt models list`

```text
List models for a workstream

Usage: list --workstream <WORKSTREAM>

Options:
      --workstream <WORKSTREAM>
          Workstream to inspect models for

  -h, --help
          Print help
```

##### `tt app tt spawn`

```text
Spawn a role-backed TT thread

Usage: spawn [OPTIONS] <ROLE>

Arguments:
  <ROLE>
          Role to use for the spawned thread

Options:
      --workstream <WORKSTREAM>
          Existing workstream to attach the spawned thread to

      --new-workstream <NEW_WORKSTREAM>
          Create a new workstream for the spawned thread

      --repo-root <REPO_ROOT>
          Repository root to bind the spawned thread to

      --headless
          Spawn the thread without a visible UI

      --model <MODEL>
          Model to use for the spawned thread

  -h, --help
          Print help
```

##### `tt app tt resume`

```text
Resume a TT thread

Usage: resume [OPTIONS] <THREAD>

Arguments:
  <THREAD>
          Thread id to resume

Options:
      --cwd <CWD>
          Working directory to resume the thread in

      --model <MODEL>
          Model to use while resuming the thread

  -h, --help
          Print help
```

##### `tt app tt worktree`

```text
Manage TT worktrees

Usage: worktree <COMMAND>

Commands:
  add    Create a new TT-managed worktree
  prune  Prune a TT-managed worktree
  help   Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

###### `tt app tt worktree add`

```text
Create a new TT-managed worktree

Usage: add <REPO_ROOT> <NAME>

Arguments:
  <REPO_ROOT>
          Repository root to add the worktree under

  <NAME>
          Worktree name to create

Options:
  -h, --help
          Print help
```

###### `tt app tt worktree prune`

```text
Prune a TT-managed worktree

Usage: prune <SELECTOR>

Arguments:
  <SELECTOR>
          Worktree selector to prune

Options:
  -h, --help
          Print help
```

##### `tt app tt threads`

```text
Manage TT thread records

Usage: threads <COMMAND>

Commands:
  list         List threads for a workstream
  list-loaded  List loaded threads for a workstream
  read         Read a thread by id
  start        Start a new thread
  resume       Resume an existing thread
  help         Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

###### `tt app tt threads list`

```text
List threads for a workstream

Usage: list --workstream <WORKSTREAM>

Options:
      --workstream <WORKSTREAM>
          Workstream to list threads for

  -h, --help
          Print help
```

###### `tt app tt threads list-loaded`

```text
List loaded threads for a workstream

Usage: list-loaded --workstream <WORKSTREAM>

Options:
      --workstream <WORKSTREAM>
          Workstream to list threads for

  -h, --help
          Print help
```

###### `tt app tt threads read`

```text
Read a thread by id

Usage: read --thread <THREAD>

Options:
      --thread <THREAD>
          Thread id to inspect

  -h, --help
          Print help
```

###### `tt app tt threads start`

```text
Start a new thread

Usage: start [OPTIONS]

Options:
      --cwd <CWD>
          Working directory to start the thread in

      --model <MODEL>
          Model to use for the spawned thread

      --ephemeral
          Start the thread without a visible UI

  -h, --help
          Print help
```

###### `tt app tt threads resume`

```text
Resume an existing thread

Usage: resume [OPTIONS] --thread <THREAD>

Options:
      --thread <THREAD>
          Thread id to resume

      --cwd <CWD>
          Working directory to resume the thread in

      --model <MODEL>
          Model to use while resuming the thread

  -h, --help
          Print help
```

##### `tt app tt turns`

```text
Inspect TT turns

Usage: turns <COMMAND>

Commands:
  list-active  List the active turns
  recent       Show recent turns for a thread
  get          Get a specific turn by thread and turn id
  help         Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

###### `tt app tt turns list-active`

```text
List the active turns

Usage: list-active

Options:
  -h, --help
          Print help
```

###### `tt app tt turns recent`

```text
Show recent turns for a thread

Usage: recent [OPTIONS] --thread <THREAD>

Options:
      --thread <THREAD>
          Thread id to inspect recent turns for

      --limit <LIMIT>
          Maximum number of turns to return
          
          [default: 10]

  -h, --help
          Print help
```

###### `tt app tt turns get`

```text
Get a specific turn by thread and turn id

Usage: get --thread <THREAD> --turn <TURN>

Options:
      --thread <THREAD>
          Thread id to read from

      --turn <TURN>
          Turn id to inspect

  -h, --help
          Print help
```

### `tt i3`

```text
Coordinate the desktop window manager

Usage: i3 <COMMAND>

Commands:
  status  Report desktop/window-manager status
  start   Start desktop/window-manager integration
  attach  Attach to the current desktop/window-manager session
  help    Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

#### `tt i3 status`

```text
Report desktop/window-manager status

Usage: status

Options:
  -h, --help
          Print help
```

#### `tt i3 start`

```text
Start desktop/window-manager integration

Usage: start

Options:
  -h, --help
          Print help
```

#### `tt i3 attach`

```text
Attach to the current desktop/window-manager session

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
  agent     Run an agent lifecycle command
  i3        Run an i3/window-manager command
  tt        Run a TT lifecycle command
  process   Run a process management command
  services  Run a managed-service command
  git       Run a git command
  apply     Apply a snapshot-scoped skill patch
  help      Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

#### `tt skill agent`

```text
Run an agent lifecycle command

Usage: agent <COMMAND>

Commands:
  spawn    Spawn an agent thread
  inspect  Inspect agent state
  resume   Resume an agent thread
  retire   Retire an agent thread
  help     Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

##### `tt skill agent spawn`

```text
Spawn an agent thread

Usage: spawn [OPTIONS] [ROLE]

Arguments:
  [ROLE]
          Role name for the spawned agent
          
          [default: agent]

Options:
      --workstream <WORKSTREAM>
          Existing workstream to attach the agent to

      --new-workstream <NEW_WORKSTREAM>
          Create a new workstream for the agent

      --repo-root <REPO_ROOT>
          Repository root to bind the agent to

      --headless
          Spawn the agent without a visible UI

      --model <MODEL>
          Model to use for the agent

  -h, --help
          Print help
```

##### `tt skill agent inspect`

```text
Inspect agent state

Usage: inspect [OPTIONS]

Options:
      --thread <THREAD>
          Thread id to inspect

      --workstream <WORKSTREAM>
          Workstream id to inspect

  -h, --help
          Print help
```

##### `tt skill agent resume`

```text
Resume an agent thread

Usage: resume [OPTIONS] <THREAD>

Arguments:
  <THREAD>
          Thread id to resume

Options:
      --cwd <CWD>
          Working directory to resume in

      --model <MODEL>
          Model to use while resuming

  -h, --help
          Print help
```

##### `tt skill agent retire`

```text
Retire an agent thread

Usage: retire [OPTIONS] <THREAD>

Arguments:
  <THREAD>
          Thread id to retire

Options:
      --note <NOTE>
          Optional retirement note

  -h, --help
          Print help
```

#### `tt skill i3`

```text
Run an i3/window-manager command

Usage: i3 <COMMAND>

Commands:
  status     Report i3/sway status
  attach     Attach to the current i3/sway session
  focus      Focus a workspace
  workspace  Inspect i3/sway workspaces
  window     Inspect i3/sway windows
  message    Send a window-manager message
  help       Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

##### `tt skill i3 status`

```text
Report i3/sway status

Usage: status

Options:
  -h, --help
          Print help
```

##### `tt skill i3 attach`

```text
Attach to the current i3/sway session

Usage: attach

Options:
  -h, --help
          Print help
```

##### `tt skill i3 focus`

```text
Focus a workspace

Usage: focus [OPTIONS]

Options:
      --workspace <WORKSPACE>
          Workspace to focus

  -h, --help
          Print help
```

##### `tt skill i3 workspace`

```text
Inspect i3/sway workspaces

Usage: workspace <COMMAND>

Commands:
  focus  Focus a workspace
  move   Move a workspace
  list   List workspaces
  help   Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

###### `tt skill i3 workspace focus`

```text
Focus a workspace

Usage: focus --workspace <WORKSPACE>

Options:
      --workspace <WORKSPACE>
          Workspace to operate on

  -h, --help
          Print help
```

###### `tt skill i3 workspace move`

```text
Move a workspace

Usage: move --workspace <WORKSPACE>

Options:
      --workspace <WORKSPACE>
          Workspace to operate on

  -h, --help
          Print help
```

###### `tt skill i3 workspace list`

```text
List workspaces

Usage: list

Options:
  -h, --help
          Print help
```

##### `tt skill i3 window`

```text
Inspect i3/sway windows

Usage: window <COMMAND>

Commands:
  focus  Focus a window
  move   Move a window
  close  Close a window
  info   Inspect a window
  help   Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

###### `tt skill i3 window focus`

```text
Focus a window

Usage: focus --criteria <CRITERIA>

Options:
      --criteria <CRITERIA>
          Window criteria used to select the target

  -h, --help
          Print help
```

###### `tt skill i3 window move`

```text
Move a window

Usage: move --criteria <CRITERIA> --workspace <WORKSPACE>

Options:
      --criteria <CRITERIA>
          Window criteria used to select the target

      --workspace <WORKSPACE>
          Workspace to move the window to

  -h, --help
          Print help
```

###### `tt skill i3 window close`

```text
Close a window

Usage: close --criteria <CRITERIA>

Options:
      --criteria <CRITERIA>
          Window criteria used to select the target

  -h, --help
          Print help
```

###### `tt skill i3 window info`

```text
Inspect a window

Usage: info --criteria <CRITERIA>

Options:
      --criteria <CRITERIA>
          Window criteria used to select the target

  -h, --help
          Print help
```

##### `tt skill i3 message`

```text
Send a window-manager message

Usage: message [MESSAGE]...

Arguments:
  [MESSAGE]...
          Message payload to send to i3/sway

Options:
  -h, --help
          Print help
```

#### `tt skill tt`

```text
Run a TT lifecycle command

Usage: tt <COMMAND>

Commands:
  status      Show TT status
  spawn       Spawn a TT thread
  resume      Resume a TT thread
  app-server  Manage a TT app-server instance
  help        Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

##### `tt skill tt status`

```text
Show TT status

Usage: status

Options:
  -h, --help
          Print help
```

##### `tt skill tt spawn`

```text
Spawn a TT thread

Usage: spawn [OPTIONS] <ROLE>

Arguments:
  <ROLE>
          Role name for the spawned TT thread

Options:
      --workstream <WORKSTREAM>
          Existing workstream to attach the TT thread to

      --new-workstream <NEW_WORKSTREAM>
          Create a new workstream for the TT thread

      --repo-root <REPO_ROOT>
          Repository root to bind the TT thread to

      --headless
          Spawn the TT thread without a visible UI

      --model <MODEL>
          Model to use for the TT thread

  -h, --help
          Print help
```

##### `tt skill tt resume`

```text
Resume a TT thread

Usage: resume [OPTIONS] <THREAD>

Arguments:
  <THREAD>
          Thread id to resume

Options:
      --cwd <CWD>
          Working directory to resume in

      --model <MODEL>
          Model to use while resuming

  -h, --help
          Print help
```

##### `tt skill tt app-server`

```text
Manage a TT app-server instance

Usage: app-server <COMMAND>

Commands:
  status   Show app-server status
  start    Start an app-server instance
  stop     Stop an app-server instance
  restart  Restart an app-server instance
  help     Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

###### `tt skill tt app-server status`

```text
Show app-server status

Usage: status [NAME]

Arguments:
  [NAME]
          Named app-server instance
          
          [default: default]

Options:
  -h, --help
          Print help
```

###### `tt skill tt app-server start`

```text
Start an app-server instance

Usage: start [NAME]

Arguments:
  [NAME]
          Named app-server instance
          
          [default: default]

Options:
  -h, --help
          Print help
```

###### `tt skill tt app-server stop`

```text
Stop an app-server instance

Usage: stop [NAME]

Arguments:
  [NAME]
          Named app-server instance
          
          [default: default]

Options:
  -h, --help
          Print help
```

###### `tt skill tt app-server restart`

```text
Restart an app-server instance

Usage: restart [NAME]

Arguments:
  [NAME]
          Named app-server instance
          
          [default: default]

Options:
  -h, --help
          Print help
```

#### `tt skill process`

```text
Run a process management command

Usage: process <COMMAND>

Commands:
  status   Show process status
  inspect  Inspect a process
  start    Start a process
  stop     Stop a process
  restart  Restart a process
  signal   Send a signal to a process
  tree     Show the process tree
  help     Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

##### `tt skill process status`

```text
Show process status

Usage: status [OPTIONS]

Options:
      --pid <PID>
          Process id to target

      --name <NAME>
          Process name to target

  -h, --help
          Print help
```

##### `tt skill process inspect`

```text
Inspect a process

Usage: inspect [OPTIONS]

Options:
      --pid <PID>
          Process id to target

      --name <NAME>
          Process name to target

  -h, --help
          Print help
```

##### `tt skill process start`

```text
Start a process

Usage: start [OPTIONS] [COMMAND]...

Arguments:
  [COMMAND]...
          Command to execute

Options:
      --pid <PID>
          Process id to start or restart

      --name <NAME>
          Process name to start or restart

      --cwd <CWD>
          Working directory for the process

  -h, --help
          Print help
```

##### `tt skill process stop`

```text
Stop a process

Usage: stop [OPTIONS]

Options:
      --pid <PID>
          Process id to target

      --name <NAME>
          Process name to target

  -h, --help
          Print help
```

##### `tt skill process restart`

```text
Restart a process

Usage: restart [OPTIONS] [COMMAND]...

Arguments:
  [COMMAND]...
          Command to execute

Options:
      --pid <PID>
          Process id to start or restart

      --name <NAME>
          Process name to start or restart

      --cwd <CWD>
          Working directory for the process

  -h, --help
          Print help
```

##### `tt skill process signal`

```text
Send a signal to a process

Usage: signal [OPTIONS]

Options:
      --pid <PID>
          Process id to signal

      --name <NAME>
          Process name to signal

      --signal <SIGNAL>
          Signal name to send
          
          [default: TERM]

  -h, --help
          Print help
```

##### `tt skill process tree`

```text
Show the process tree

Usage: tree [OPTIONS]

Options:
      --pid <PID>
          Process id to target

      --name <NAME>
          Process name to target

  -h, --help
          Print help
```

#### `tt skill services`

```text
Run a managed-service command

Usage: services <COMMAND>

Commands:
  status   Show managed-service status
  inspect  Inspect a managed service
  start    Start a managed service
  stop     Stop a managed service
  restart  Restart a managed service
  reload   Reload a managed service
  help     Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

##### `tt skill services status`

```text
Show managed-service status

Usage: status <SERVICE>

Arguments:
  <SERVICE>
          Managed service to operate on
          
          [possible values: daemon, app-server]

Options:
  -h, --help
          Print help
```

##### `tt skill services inspect`

```text
Inspect a managed service

Usage: inspect <SERVICE>

Arguments:
  <SERVICE>
          Managed service to operate on
          
          [possible values: daemon, app-server]

Options:
  -h, --help
          Print help
```

##### `tt skill services start`

```text
Start a managed service

Usage: start <SERVICE>

Arguments:
  <SERVICE>
          Managed service to operate on
          
          [possible values: daemon, app-server]

Options:
  -h, --help
          Print help
```

##### `tt skill services stop`

```text
Stop a managed service

Usage: stop <SERVICE>

Arguments:
  <SERVICE>
          Managed service to operate on
          
          [possible values: daemon, app-server]

Options:
  -h, --help
          Print help
```

##### `tt skill services restart`

```text
Restart a managed service

Usage: restart <SERVICE>

Arguments:
  <SERVICE>
          Managed service to operate on
          
          [possible values: daemon, app-server]

Options:
  -h, --help
          Print help
```

##### `tt skill services reload`

```text
Reload a managed service

Usage: reload <SERVICE>

Arguments:
  <SERVICE>
          Managed service to operate on
          
          [possible values: daemon, app-server]

Options:
  -h, --help
          Print help
```

#### `tt skill git`

```text
Run a git command

Usage: git <COMMAND>

Commands:
  status    Show repository status
  branch    Inspect git branches
  worktree  Inspect git worktrees
  help      Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

##### `tt skill git status`

```text
Show repository status

Usage: status [OPTIONS]

Options:
      --repo-root <REPO_ROOT>
          Repository root to inspect

      --worktree-path <WORKTREE_PATH>
          Worktree path to inspect

  -h, --help
          Print help
```

##### `tt skill git branch`

```text
Inspect git branches

Usage: branch <COMMAND>

Commands:
  current  Show the current branch
  list     List branches
  help     Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

###### `tt skill git branch current`

```text
Show the current branch

Usage: current [OPTIONS]

Options:
      --repo-root <REPO_ROOT>
          Repository root to inspect

      --worktree-path <WORKTREE_PATH>
          Worktree path to inspect

  -h, --help
          Print help
```

###### `tt skill git branch list`

```text
List branches

Usage: list [OPTIONS]

Options:
      --repo-root <REPO_ROOT>
          Repository root to inspect

      --worktree-path <WORKTREE_PATH>
          Worktree path to inspect

  -h, --help
          Print help
```

##### `tt skill git worktree`

```text
Inspect git worktrees

Usage: worktree <COMMAND>

Commands:
  current  Show the current worktree
  list     List worktrees
  help     Print this message or the help of the given subcommand(s)

Options:
  -h, --help
          Print help
```

###### `tt skill git worktree current`

```text
Show the current worktree

Usage: current [OPTIONS]

Options:
      --repo-root <REPO_ROOT>
          Repository root to inspect

      --worktree-path <WORKTREE_PATH>
          Worktree path to inspect

  -h, --help
          Print help
```

###### `tt skill git worktree list`

```text
List worktrees

Usage: list [OPTIONS]

Options:
      --repo-root <REPO_ROOT>
          Repository root to inspect

      --worktree-path <WORKTREE_PATH>
          Worktree path to inspect

  -h, --help
          Print help
```

#### `tt skill apply`

```text
Apply a snapshot-scoped skill patch

Usage: apply [OPTIONS] --snapshot <SNAPSHOT_ID>

Options:
      --snapshot <SNAPSHOT_ID>
          Snapshot id to apply the skill against

      --skill <SKILLS>
          Skill id to apply

      --out <OUT>
          Optional output path for generated artifacts

  -h, --help
          Print help
```

### `tt prompt`

```text
Send a single prompt to a thread

Usage: prompt --thread <THREAD> --text <TEXT>

Options:
      --thread <THREAD>
          Target thread id to receive the prompt

      --text <TEXT>
          Prompt text to send to the thread

  -h, --help
          Print help
```

### `tt quickstart`

```text
Launch a quick TT session from freeform input

Usage: quickstart [OPTIONS] --text <TEXT>

Options:
      --cwd <CWD>
          Optional working directory for the quickstart session

      --model <MODEL>
          Optional model override for the quickstart session

      --text <TEXT>
          Freeform text used to seed the session

  -h, --help
          Print help
```

