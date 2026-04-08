use std::path::PathBuf;

use anyhow::Result;
use async_trait::async_trait;
use clap::{Args, Subcommand, ValueEnum};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SkillContext {
    pub server_url: Option<String>,
    pub operator_api_token: Option<String>,
    pub tt_bin: Option<PathBuf>,
    pub listen_url: Option<String>,
    pub inbox_mirror_server_url: Option<String>,
    pub cwd: Option<PathBuf>,
    pub worktree_root: Option<PathBuf>,
    pub model: Option<String>,
    pub connect_only: bool,
    pub force_spawn: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillOutcome {
    pub summary: String,
    pub details: Vec<String>,
}

impl SkillOutcome {
    pub fn new(summary: impl Into<String>) -> Self {
        Self {
            summary: summary.into(),
            details: Vec::new(),
        }
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.details.push(detail.into());
        self
    }
}

#[derive(Debug, Clone, Subcommand)]
#[command(rename_all = "kebab-case")]
pub enum SkillCommand {
    /// Run an agent lifecycle command.
    Agent {
        #[command(subcommand)]
        command: AgentCommand,
    },
    /// Run an i3/window-manager command.
    I3 {
        #[command(subcommand)]
        command: I3Command,
    },
    /// Run a TT lifecycle command.
    TT {
        #[command(subcommand)]
        command: TTCommand,
    },
    /// Run a process management command.
    Process {
        #[command(subcommand)]
        command: ProcessCommand,
    },
    /// Run a managed-service command.
    Services {
        #[command(subcommand)]
        command: ServicesCommand,
    },
    /// Run a git command.
    Git {
        #[command(subcommand)]
        command: GitCommand,
    },
    /// Apply a snapshot-scoped skill patch.
    Apply(SkillApplyArgs),
}

#[derive(Debug, Clone, Subcommand)]
#[command(rename_all = "kebab-case")]
pub enum AgentCommand {
    /// Spawn an agent thread.
    Spawn(AgentSpawnArgs),
    /// Inspect agent state.
    Inspect(AgentInspectArgs),
    /// Resume an agent thread.
    Resume(ResumeArgs),
    /// Retire an agent thread.
    Retire(AgentRetireArgs),
}

#[derive(Debug, Clone, Subcommand)]
#[command(rename_all = "kebab-case")]
pub enum I3Command {
    /// Report i3/sway status.
    Status(I3StatusArgs),
    /// Attach to the current i3/sway session.
    Attach(I3AttachArgs),
    /// Focus a workspace.
    Focus(I3FocusArgs),
    /// Inspect i3/sway workspaces.
    Workspace {
        #[command(subcommand)]
        command: I3WorkspaceCommand,
    },
    /// Inspect i3/sway windows.
    Window {
        #[command(subcommand)]
        command: I3WindowCommand,
    },
    /// Send a window-manager message.
    Message(I3MessageArgs),
}

#[derive(Debug, Clone, Subcommand)]
#[command(rename_all = "kebab-case")]
pub enum TTCommand {
    /// Show TT status.
    Status(TTStatusArgs),
    /// Spawn a TT thread.
    Spawn(TTSpawnArgs),
    /// Resume a TT thread.
    Resume(ResumeArgs),
    /// Manage a TT app-server instance.
    AppServer {
        #[command(subcommand)]
        command: AppServerCommand,
    },
}

#[derive(Debug, Clone, Subcommand)]
#[command(rename_all = "kebab-case")]
pub enum AppServerCommand {
    /// Show app-server status.
    Status(AppServerNameArgs),
    /// Start an app-server instance.
    Start(AppServerNameArgs),
    /// Stop an app-server instance.
    Stop(AppServerNameArgs),
    /// Restart an app-server instance.
    Restart(AppServerNameArgs),
}

#[derive(Debug, Clone, Subcommand)]
#[command(rename_all = "kebab-case")]
pub enum ProcessCommand {
    /// Show process status.
    Status(ProcessTargetArgs),
    /// Inspect a process.
    Inspect(ProcessTargetArgs),
    /// Start a process.
    Start(ProcessStartArgs),
    /// Stop a process.
    Stop(ProcessTargetArgs),
    /// Restart a process.
    Restart(ProcessStartArgs),
    /// Send a signal to a process.
    Signal(ProcessSignalArgs),
    /// Show the process tree.
    Tree(ProcessTargetArgs),
}

#[derive(Debug, Clone, Subcommand)]
#[command(rename_all = "kebab-case")]
pub enum ServicesCommand {
    /// Show managed-service status.
    Status(ManagedServiceArgs),
    /// Inspect a managed service.
    Inspect(ManagedServiceArgs),
    /// Start a managed service.
    Start(ManagedServiceArgs),
    /// Stop a managed service.
    Stop(ManagedServiceArgs),
    /// Restart a managed service.
    Restart(ManagedServiceArgs),
    /// Reload a managed service.
    Reload(ManagedServiceArgs),
}

#[derive(Debug, Clone, Subcommand)]
#[command(rename_all = "kebab-case")]
pub enum GitCommand {
    /// Show repository status.
    Status(GitRepoArgs),
    /// Inspect git branches.
    Branch {
        #[command(subcommand)]
        command: GitBranchCommand,
    },
    /// Inspect git worktrees.
    Worktree {
        #[command(subcommand)]
        command: GitWorktreeCommand,
    },
}

#[derive(Debug, Clone, Subcommand)]
#[command(rename_all = "kebab-case")]
pub enum I3WorkspaceCommand {
    /// Focus a workspace.
    Focus(I3WorkspaceArgs),
    /// Move a workspace.
    Move(I3WorkspaceArgs),
    /// List workspaces.
    List(I3ListArgs),
}

#[derive(Debug, Clone, Subcommand)]
#[command(rename_all = "kebab-case")]
pub enum I3WindowCommand {
    /// Focus a window.
    Focus(I3WindowArgs),
    /// Move a window.
    Move(I3WindowMoveArgs),
    /// Close a window.
    Close(I3WindowArgs),
    /// Inspect a window.
    Info(I3WindowArgs),
}

#[derive(Debug, Clone, Subcommand)]
#[command(rename_all = "kebab-case")]
pub enum GitBranchCommand {
    /// Show the current branch.
    Current(GitRepoArgs),
    /// List branches.
    List(GitRepoArgs),
}

#[derive(Debug, Clone, Subcommand)]
#[command(rename_all = "kebab-case")]
pub enum GitWorktreeCommand {
    /// Show the current worktree.
    Current(GitRepoArgs),
    /// List worktrees.
    List(GitRepoArgs),
}

#[derive(Debug, Clone, ValueEnum)]
#[value(rename_all = "kebab-case")]
pub enum ManagedServiceKind {
    Daemon,
    AppServer,
}

#[derive(Debug, Clone, Args)]
pub struct AgentSpawnArgs {
    #[arg(default_value = "agent", help = "Role name for the spawned agent")]
    pub role: String,
    #[arg(long, help = "Existing workstream to attach the agent to")]
    pub workstream: Option<String>,
    #[arg(
        long = "new-workstream",
        help = "Create a new workstream for the agent"
    )]
    pub new_workstream: Option<String>,
    #[arg(long, help = "Repository root to bind the agent to")]
    pub repo_root: Option<PathBuf>,
    #[arg(long, default_value_t = false, help = "Spawn the agent without a visible UI")]
    pub headless: bool,
    #[arg(long, help = "Model to use for the agent")]
    pub model: Option<String>,
}

#[derive(Debug, Clone, Args)]
pub struct TTSpawnArgs {
    /// Role name for the spawned TT thread.
    pub role: String,
    #[arg(long, help = "Existing workstream to attach the TT thread to")]
    pub workstream: Option<String>,
    #[arg(
        long = "new-workstream",
        help = "Create a new workstream for the TT thread"
    )]
    pub new_workstream: Option<String>,
    #[arg(long, help = "Repository root to bind the TT thread to")]
    pub repo_root: Option<PathBuf>,
    #[arg(long, default_value_t = false, help = "Spawn the TT thread without a visible UI")]
    pub headless: bool,
    #[arg(long, help = "Model to use for the TT thread")]
    pub model: Option<String>,
}

#[derive(Debug, Clone, Args)]
pub struct SkillApplyArgs {
    #[arg(long = "snapshot", help = "Snapshot id to apply the skill against")]
    pub snapshot_id: String,
    #[arg(long = "skill", help = "Skill id to apply")]
    pub skills: Vec<String>,
    #[arg(long, help = "Optional output path for generated artifacts")]
    pub out: Option<PathBuf>,
}

#[derive(Debug, Clone, Args)]
pub struct ResumeArgs {
    /// Thread id to resume.
    pub thread: String,
    #[arg(long, help = "Working directory to resume in")]
    pub cwd: Option<PathBuf>,
    #[arg(long, help = "Model to use while resuming")]
    pub model: Option<String>,
}

#[derive(Debug, Clone, Args, Default)]
pub struct AgentInspectArgs {
    #[arg(long, help = "Thread id to inspect")]
    pub thread: Option<String>,
    #[arg(long, help = "Workstream id to inspect")]
    pub workstream: Option<String>,
}

#[derive(Debug, Clone, Args)]
pub struct AgentRetireArgs {
    /// Thread id to retire.
    pub thread: String,
    #[arg(long, help = "Optional retirement note")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Args, Default)]
pub struct I3StatusArgs {}

#[derive(Debug, Clone, Args, Default)]
pub struct I3AttachArgs {}

#[derive(Debug, Clone, Args, Default)]
pub struct I3FocusArgs {
    #[arg(long, help = "Workspace to focus")]
    pub workspace: Option<String>,
}

#[derive(Debug, Clone, Args, Default)]
pub struct I3WorkspaceArgs {
    #[arg(long, help = "Workspace to operate on")]
    pub workspace: String,
}

#[derive(Debug, Clone, Args)]
pub struct I3WindowArgs {
    #[arg(long, help = "Window criteria used to select the target")]
    pub criteria: String,
}

#[derive(Debug, Clone, Args)]
pub struct I3WindowMoveArgs {
    #[arg(long, help = "Window criteria used to select the target")]
    pub criteria: String,
    #[arg(long, help = "Workspace to move the window to")]
    pub workspace: String,
}

#[derive(Debug, Clone, Args)]
pub struct I3MessageArgs {
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, help = "Message payload to send to i3/sway")]
    pub message: Vec<String>,
}

#[derive(Debug, Clone, Args, Default)]
pub struct TTStatusArgs {}

#[derive(Debug, Clone, Args)]
pub struct AppServerNameArgs {
    #[arg(default_value = "default", help = "Named app-server instance")]
    pub name: String,
}

#[derive(Debug, Clone, Args)]
pub struct ProcessTargetArgs {
    #[arg(long, help = "Process id to target")]
    pub pid: Option<u32>,
    #[arg(long, help = "Process name to target")]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Args)]
pub struct ProcessStartArgs {
    #[arg(long, help = "Process id to start or restart")]
    pub pid: Option<u32>,
    #[arg(long, help = "Process name to start or restart")]
    pub name: Option<String>,
    #[arg(long, help = "Working directory for the process")]
    pub cwd: Option<PathBuf>,
    #[arg(
        trailing_var_arg = true,
        allow_hyphen_values = true,
        help = "Command to execute"
    )]
    pub command: Vec<String>,
}

#[derive(Debug, Clone, Args)]
pub struct ProcessSignalArgs {
    #[arg(long, help = "Process id to signal")]
    pub pid: Option<u32>,
    #[arg(long, help = "Process name to signal")]
    pub name: Option<String>,
    #[arg(long, default_value = "TERM", help = "Signal name to send")]
    pub signal: String,
}

#[derive(Debug, Clone, Args)]
pub struct ManagedServiceArgs {
    #[arg(value_enum, help = "Managed service to operate on")]
    pub service: ManagedServiceKind,
}

#[derive(Debug, Clone, Args)]
pub struct GitRepoArgs {
    #[arg(long, help = "Repository root to inspect")]
    pub repo_root: Option<PathBuf>,
    #[arg(long, help = "Worktree path to inspect")]
    pub worktree_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Args, Default)]
pub struct I3ListArgs {}

#[async_trait(?Send)]
pub trait SkillBackend: Send + Sync {
    async fn agent_spawn(
        &self,
        context: &SkillContext,
        args: &AgentSpawnArgs,
    ) -> Result<SkillOutcome>;
    async fn agent_inspect(
        &self,
        context: &SkillContext,
        args: &AgentInspectArgs,
    ) -> Result<SkillOutcome>;
    async fn agent_resume(&self, context: &SkillContext, args: &ResumeArgs)
    -> Result<SkillOutcome>;
    async fn agent_retire(
        &self,
        context: &SkillContext,
        args: &AgentRetireArgs,
    ) -> Result<SkillOutcome>;

    async fn i3_status(&self, context: &SkillContext, args: &I3StatusArgs) -> Result<SkillOutcome>;
    async fn i3_attach(&self, context: &SkillContext, args: &I3AttachArgs) -> Result<SkillOutcome>;
    async fn i3_focus(&self, context: &SkillContext, args: &I3FocusArgs) -> Result<SkillOutcome>;

    async fn tt_status(&self, context: &SkillContext, args: &TTStatusArgs) -> Result<SkillOutcome>;
    async fn tt_spawn(&self, context: &SkillContext, args: &TTSpawnArgs) -> Result<SkillOutcome>;
    async fn tt_resume(&self, context: &SkillContext, args: &ResumeArgs) -> Result<SkillOutcome>;
    async fn tt_app_server_status(
        &self,
        context: &SkillContext,
        args: &AppServerNameArgs,
    ) -> Result<SkillOutcome>;
    async fn tt_app_server_start(
        &self,
        context: &SkillContext,
        args: &AppServerNameArgs,
    ) -> Result<SkillOutcome>;
    async fn tt_app_server_stop(
        &self,
        context: &SkillContext,
        args: &AppServerNameArgs,
    ) -> Result<SkillOutcome>;
    async fn tt_app_server_restart(
        &self,
        context: &SkillContext,
        args: &AppServerNameArgs,
    ) -> Result<SkillOutcome>;

    async fn process_status(
        &self,
        context: &SkillContext,
        args: &ProcessTargetArgs,
    ) -> Result<SkillOutcome>;
    async fn process_inspect(
        &self,
        context: &SkillContext,
        args: &ProcessTargetArgs,
    ) -> Result<SkillOutcome>;
    async fn process_start(
        &self,
        context: &SkillContext,
        args: &ProcessStartArgs,
    ) -> Result<SkillOutcome>;
    async fn process_stop(
        &self,
        context: &SkillContext,
        args: &ProcessTargetArgs,
    ) -> Result<SkillOutcome>;
    async fn process_restart(
        &self,
        context: &SkillContext,
        args: &ProcessStartArgs,
    ) -> Result<SkillOutcome>;
    async fn process_signal(
        &self,
        context: &SkillContext,
        args: &ProcessSignalArgs,
    ) -> Result<SkillOutcome>;
    async fn process_tree(
        &self,
        context: &SkillContext,
        args: &ProcessTargetArgs,
    ) -> Result<SkillOutcome>;

    async fn services_status(
        &self,
        context: &SkillContext,
        args: &ManagedServiceArgs,
    ) -> Result<SkillOutcome>;
    async fn services_inspect(
        &self,
        context: &SkillContext,
        args: &ManagedServiceArgs,
    ) -> Result<SkillOutcome>;
    async fn services_start(
        &self,
        context: &SkillContext,
        args: &ManagedServiceArgs,
    ) -> Result<SkillOutcome>;
    async fn services_stop(
        &self,
        context: &SkillContext,
        args: &ManagedServiceArgs,
    ) -> Result<SkillOutcome>;
    async fn services_restart(
        &self,
        context: &SkillContext,
        args: &ManagedServiceArgs,
    ) -> Result<SkillOutcome>;
    async fn services_reload(
        &self,
        context: &SkillContext,
        args: &ManagedServiceArgs,
    ) -> Result<SkillOutcome>;

    async fn git_status(&self, context: &SkillContext, args: &GitRepoArgs) -> Result<SkillOutcome>;
    async fn git_branch_current(
        &self,
        context: &SkillContext,
        args: &GitRepoArgs,
    ) -> Result<SkillOutcome>;
    async fn git_branch_list(
        &self,
        context: &SkillContext,
        args: &GitRepoArgs,
    ) -> Result<SkillOutcome>;
    async fn git_worktree_current(
        &self,
        context: &SkillContext,
        args: &GitRepoArgs,
    ) -> Result<SkillOutcome>;
    async fn git_worktree_list(
        &self,
        context: &SkillContext,
        args: &GitRepoArgs,
    ) -> Result<SkillOutcome>;

    async fn i3_workspace_focus(
        &self,
        context: &SkillContext,
        args: &I3WorkspaceArgs,
    ) -> Result<SkillOutcome>;
    async fn i3_workspace_move(
        &self,
        context: &SkillContext,
        args: &I3WorkspaceArgs,
    ) -> Result<SkillOutcome>;
    async fn i3_window_focus(
        &self,
        context: &SkillContext,
        args: &I3WindowArgs,
    ) -> Result<SkillOutcome>;
    async fn i3_window_move(
        &self,
        context: &SkillContext,
        args: &I3WindowMoveArgs,
    ) -> Result<SkillOutcome>;
    async fn i3_window_close(
        &self,
        context: &SkillContext,
        args: &I3WindowArgs,
    ) -> Result<SkillOutcome>;
    async fn i3_window_info(
        &self,
        context: &SkillContext,
        args: &I3WindowArgs,
    ) -> Result<SkillOutcome>;
    async fn i3_message(
        &self,
        context: &SkillContext,
        args: &I3MessageArgs,
    ) -> Result<SkillOutcome>;
    async fn i3_workspace_list(
        &self,
        context: &SkillContext,
        args: &I3ListArgs,
    ) -> Result<SkillOutcome>;
    async fn skill_apply(
        &self,
        context: &SkillContext,
        args: &SkillApplyArgs,
    ) -> Result<SkillOutcome>;
}

pub async fn dispatch<B: SkillBackend + ?Sized>(
    backend: &B,
    context: &SkillContext,
    command: SkillCommand,
) -> Result<SkillOutcome> {
    match command {
        SkillCommand::Agent { command } => match command {
            AgentCommand::Spawn(args) => backend.agent_spawn(context, &args).await,
            AgentCommand::Inspect(args) => backend.agent_inspect(context, &args).await,
            AgentCommand::Resume(args) => backend.agent_resume(context, &args).await,
            AgentCommand::Retire(args) => backend.agent_retire(context, &args).await,
        },
        SkillCommand::I3 { command } => match command {
            I3Command::Status(args) => backend.i3_status(context, &args).await,
            I3Command::Attach(args) => backend.i3_attach(context, &args).await,
            I3Command::Focus(args) => backend.i3_focus(context, &args).await,
            I3Command::Workspace { command } => match command {
                I3WorkspaceCommand::Focus(args) => backend.i3_workspace_focus(context, &args).await,
                I3WorkspaceCommand::Move(args) => backend.i3_workspace_move(context, &args).await,
                I3WorkspaceCommand::List(args) => backend.i3_workspace_list(context, &args).await,
            },
            I3Command::Window { command } => match command {
                I3WindowCommand::Focus(args) => backend.i3_window_focus(context, &args).await,
                I3WindowCommand::Move(args) => backend.i3_window_move(context, &args).await,
                I3WindowCommand::Close(args) => backend.i3_window_close(context, &args).await,
                I3WindowCommand::Info(args) => backend.i3_window_info(context, &args).await,
            },
            I3Command::Message(args) => backend.i3_message(context, &args).await,
        },
        SkillCommand::TT { command } => match command {
            TTCommand::Status(args) => backend.tt_status(context, &args).await,
            TTCommand::Spawn(args) => backend.tt_spawn(context, &args).await,
            TTCommand::Resume(args) => backend.tt_resume(context, &args).await,
            TTCommand::AppServer { command } => match command {
                AppServerCommand::Status(args) => {
                    backend.tt_app_server_status(context, &args).await
                }
                AppServerCommand::Start(args) => backend.tt_app_server_start(context, &args).await,
                AppServerCommand::Stop(args) => backend.tt_app_server_stop(context, &args).await,
                AppServerCommand::Restart(args) => {
                    backend.tt_app_server_restart(context, &args).await
                }
            },
        },
        SkillCommand::Process { command } => match command {
            ProcessCommand::Status(args) => backend.process_status(context, &args).await,
            ProcessCommand::Inspect(args) => backend.process_inspect(context, &args).await,
            ProcessCommand::Start(args) => backend.process_start(context, &args).await,
            ProcessCommand::Stop(args) => backend.process_stop(context, &args).await,
            ProcessCommand::Restart(args) => backend.process_restart(context, &args).await,
            ProcessCommand::Signal(args) => backend.process_signal(context, &args).await,
            ProcessCommand::Tree(args) => backend.process_tree(context, &args).await,
        },
        SkillCommand::Services { command } => match command {
            ServicesCommand::Status(args) => backend.services_status(context, &args).await,
            ServicesCommand::Inspect(args) => backend.services_inspect(context, &args).await,
            ServicesCommand::Start(args) => backend.services_start(context, &args).await,
            ServicesCommand::Stop(args) => backend.services_stop(context, &args).await,
            ServicesCommand::Restart(args) => backend.services_restart(context, &args).await,
            ServicesCommand::Reload(args) => backend.services_reload(context, &args).await,
        },
        SkillCommand::Git { command } => match command {
            GitCommand::Status(args) => backend.git_status(context, &args).await,
            GitCommand::Branch { command } => match command {
                GitBranchCommand::Current(args) => backend.git_branch_current(context, &args).await,
                GitBranchCommand::List(args) => backend.git_branch_list(context, &args).await,
            },
            GitCommand::Worktree { command } => match command {
                GitWorktreeCommand::Current(args) => {
                    backend.git_worktree_current(context, &args).await
                }
                GitWorktreeCommand::List(args) => backend.git_worktree_list(context, &args).await,
            },
        },
        SkillCommand::Apply(args) => backend.skill_apply(context, &args).await,
    }
}

#[cfg(test)]
mod tests2 {
    use super::*;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct StubBackend {
        calls: Arc<Mutex<Vec<String>>>,
    }

    impl StubBackend {
        fn push(&self, call: impl Into<String>) {
            self.calls.lock().expect("lock call log").push(call.into());
        }
    }

    #[async_trait(?Send)]
    impl SkillBackend for StubBackend {
        async fn agent_spawn(&self, _: &SkillContext, _: &AgentSpawnArgs) -> Result<SkillOutcome> {
            self.push("agent.spawn");
            Ok(SkillOutcome::new("agent.spawn"))
        }

        async fn agent_inspect(
            &self,
            _: &SkillContext,
            _: &AgentInspectArgs,
        ) -> Result<SkillOutcome> {
            self.push("agent.inspect");
            Ok(SkillOutcome::new("agent.inspect"))
        }

        async fn agent_resume(&self, _: &SkillContext, _: &ResumeArgs) -> Result<SkillOutcome> {
            self.push("agent.resume");
            Ok(SkillOutcome::new("agent.resume"))
        }

        async fn agent_retire(
            &self,
            _: &SkillContext,
            _: &AgentRetireArgs,
        ) -> Result<SkillOutcome> {
            self.push("agent.retire");
            Ok(SkillOutcome::new("agent.retire"))
        }

        async fn i3_status(&self, _: &SkillContext, _: &I3StatusArgs) -> Result<SkillOutcome> {
            self.push("i3.status");
            Ok(SkillOutcome::new("i3.status"))
        }

        async fn i3_attach(&self, _: &SkillContext, _: &I3AttachArgs) -> Result<SkillOutcome> {
            self.push("i3.attach");
            Ok(SkillOutcome::new("i3.attach"))
        }

        async fn i3_focus(&self, _: &SkillContext, _: &I3FocusArgs) -> Result<SkillOutcome> {
            self.push("i3.focus");
            Ok(SkillOutcome::new("i3.focus"))
        }

        async fn i3_workspace_focus(
            &self,
            _: &SkillContext,
            _: &I3WorkspaceArgs,
        ) -> Result<SkillOutcome> {
            self.push("i3.workspace.focus");
            Ok(SkillOutcome::new("i3.workspace.focus"))
        }

        async fn i3_workspace_move(
            &self,
            _: &SkillContext,
            _: &I3WorkspaceArgs,
        ) -> Result<SkillOutcome> {
            self.push("i3.workspace.move");
            Ok(SkillOutcome::new("i3.workspace.move"))
        }

        async fn i3_window_focus(
            &self,
            _: &SkillContext,
            _: &I3WindowArgs,
        ) -> Result<SkillOutcome> {
            self.push("i3.window.focus");
            Ok(SkillOutcome::new("i3.window.focus"))
        }

        async fn i3_window_move(
            &self,
            _: &SkillContext,
            _: &I3WindowMoveArgs,
        ) -> Result<SkillOutcome> {
            self.push("i3.window.move");
            Ok(SkillOutcome::new("i3.window.move"))
        }

        async fn i3_window_close(
            &self,
            _: &SkillContext,
            _: &I3WindowArgs,
        ) -> Result<SkillOutcome> {
            self.push("i3.window.close");
            Ok(SkillOutcome::new("i3.window.close"))
        }

        async fn i3_message(&self, _: &SkillContext, _: &I3MessageArgs) -> Result<SkillOutcome> {
            self.push("i3.message");
            Ok(SkillOutcome::new("i3.message"))
        }

        async fn tt_status(&self, _: &SkillContext, _: &TTStatusArgs) -> Result<SkillOutcome> {
            self.push("tt.status");
            Ok(SkillOutcome::new("tt.status"))
        }

        async fn tt_spawn(&self, _: &SkillContext, _: &TTSpawnArgs) -> Result<SkillOutcome> {
            self.push("tt.spawn");
            Ok(SkillOutcome::new("tt.spawn"))
        }

        async fn tt_resume(&self, _: &SkillContext, _: &ResumeArgs) -> Result<SkillOutcome> {
            self.push("tt.resume");
            Ok(SkillOutcome::new("tt.resume"))
        }

        async fn tt_app_server_status(
            &self,
            _: &SkillContext,
            _: &AppServerNameArgs,
        ) -> Result<SkillOutcome> {
            self.push("tt.app_server.status");
            Ok(SkillOutcome::new("tt.app_server.status"))
        }

        async fn tt_app_server_start(
            &self,
            _: &SkillContext,
            _: &AppServerNameArgs,
        ) -> Result<SkillOutcome> {
            self.push("tt.app_server.start");
            Ok(SkillOutcome::new("tt.app_server.start"))
        }

        async fn tt_app_server_stop(
            &self,
            _: &SkillContext,
            _: &AppServerNameArgs,
        ) -> Result<SkillOutcome> {
            self.push("tt.app_server.stop");
            Ok(SkillOutcome::new("tt.app_server.stop"))
        }

        async fn tt_app_server_restart(
            &self,
            _: &SkillContext,
            _: &AppServerNameArgs,
        ) -> Result<SkillOutcome> {
            self.push("tt.app_server.restart");
            Ok(SkillOutcome::new("tt.app_server.restart"))
        }

        async fn process_status(
            &self,
            _: &SkillContext,
            _: &ProcessTargetArgs,
        ) -> Result<SkillOutcome> {
            self.push("process.status");
            Ok(SkillOutcome::new("process.status"))
        }

        async fn process_inspect(
            &self,
            _: &SkillContext,
            _: &ProcessTargetArgs,
        ) -> Result<SkillOutcome> {
            self.push("process.inspect");
            Ok(SkillOutcome::new("process.inspect"))
        }

        async fn process_start(
            &self,
            _: &SkillContext,
            _: &ProcessStartArgs,
        ) -> Result<SkillOutcome> {
            self.push("process.start");
            Ok(SkillOutcome::new("process.start"))
        }

        async fn process_stop(
            &self,
            _: &SkillContext,
            _: &ProcessTargetArgs,
        ) -> Result<SkillOutcome> {
            self.push("process.stop");
            Ok(SkillOutcome::new("process.stop"))
        }

        async fn process_restart(
            &self,
            _: &SkillContext,
            _: &ProcessStartArgs,
        ) -> Result<SkillOutcome> {
            self.push("process.restart");
            Ok(SkillOutcome::new("process.restart"))
        }

        async fn process_signal(
            &self,
            _: &SkillContext,
            _: &ProcessSignalArgs,
        ) -> Result<SkillOutcome> {
            self.push("process.signal");
            Ok(SkillOutcome::new("process.signal"))
        }

        async fn process_tree(
            &self,
            _: &SkillContext,
            _: &ProcessTargetArgs,
        ) -> Result<SkillOutcome> {
            self.push("process.tree");
            Ok(SkillOutcome::new("process.tree"))
        }

        async fn services_status(
            &self,
            _: &SkillContext,
            _: &ManagedServiceArgs,
        ) -> Result<SkillOutcome> {
            self.push("services.status");
            Ok(SkillOutcome::new("services.status"))
        }

        async fn services_inspect(
            &self,
            _: &SkillContext,
            _: &ManagedServiceArgs,
        ) -> Result<SkillOutcome> {
            self.push("services.inspect");
            Ok(SkillOutcome::new("services.inspect"))
        }

        async fn services_start(
            &self,
            _: &SkillContext,
            _: &ManagedServiceArgs,
        ) -> Result<SkillOutcome> {
            self.push("services.start");
            Ok(SkillOutcome::new("services.start"))
        }

        async fn services_stop(
            &self,
            _: &SkillContext,
            _: &ManagedServiceArgs,
        ) -> Result<SkillOutcome> {
            self.push("services.stop");
            Ok(SkillOutcome::new("services.stop"))
        }

        async fn services_restart(
            &self,
            _: &SkillContext,
            _: &ManagedServiceArgs,
        ) -> Result<SkillOutcome> {
            self.push("services.restart");
            Ok(SkillOutcome::new("services.restart"))
        }

        async fn services_reload(
            &self,
            _: &SkillContext,
            _: &ManagedServiceArgs,
        ) -> Result<SkillOutcome> {
            self.push("services.reload");
            Ok(SkillOutcome::new("services.reload"))
        }

        async fn git_status(&self, _: &SkillContext, _: &GitRepoArgs) -> Result<SkillOutcome> {
            self.push("git.status");
            Ok(SkillOutcome::new("git.status"))
        }

        async fn git_branch_current(
            &self,
            _: &SkillContext,
            _: &GitRepoArgs,
        ) -> Result<SkillOutcome> {
            self.push("git.branch.current");
            Ok(SkillOutcome::new("git.branch.current"))
        }

        async fn git_branch_list(&self, _: &SkillContext, _: &GitRepoArgs) -> Result<SkillOutcome> {
            self.push("git.branch.list");
            Ok(SkillOutcome::new("git.branch.list"))
        }

        async fn git_worktree_current(
            &self,
            _: &SkillContext,
            _: &GitRepoArgs,
        ) -> Result<SkillOutcome> {
            self.push("git.worktree.current");
            Ok(SkillOutcome::new("git.worktree.current"))
        }

        async fn git_worktree_list(
            &self,
            _: &SkillContext,
            _: &GitRepoArgs,
        ) -> Result<SkillOutcome> {
            self.push("git.worktree.list");
            Ok(SkillOutcome::new("git.worktree.list"))
        }

        async fn i3_window_info(&self, _: &SkillContext, _: &I3WindowArgs) -> Result<SkillOutcome> {
            self.push("i3.window.info");
            Ok(SkillOutcome::new("i3.window.info"))
        }

        async fn i3_workspace_list(
            &self,
            _: &SkillContext,
            _: &I3ListArgs,
        ) -> Result<SkillOutcome> {
            self.push("i3.workspace.list");
            Ok(SkillOutcome::new("i3.workspace.list"))
        }

        async fn skill_apply(
            &self,
            _: &SkillContext,
            _: &SkillApplyArgs,
        ) -> Result<SkillOutcome> {
            self.push("skill.apply");
            Ok(SkillOutcome::new("skill.apply"))
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn dispatch_routes_new_skill_commands_again() {
        let backend = StubBackend::default();
        let context = SkillContext::default();
        let outcome = dispatch(
            &backend,
            &context,
            SkillCommand::Git {
                command: GitCommand::Worktree {
                    command: GitWorktreeCommand::Current(GitRepoArgs {
                        repo_root: Some(PathBuf::from("/tmp/repo")),
                        worktree_path: Some(PathBuf::from("/tmp/repo/worktrees/tt-1")),
                    }),
                },
            },
        )
        .await
        .expect("dispatch");
        assert_eq!(outcome.summary, "git.worktree.current");
        assert_eq!(
            backend.calls.lock().expect("lock call log").as_slice(),
            ["git.worktree.current"]
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn dispatch_routes_new_skill_commands() {
        let backend = StubBackend::default();
        let context = SkillContext::default();

        let outcome = dispatch(
            &backend,
            &context,
            SkillCommand::Process {
                command: ProcessCommand::Signal(ProcessSignalArgs {
                    pid: Some(42),
                    name: None,
                    signal: "HUP".to_string(),
                }),
            },
        )
        .await
        .expect("dispatch");
        assert_eq!(outcome.summary, "process.signal");

        let outcome = dispatch(
            &backend,
            &context,
            SkillCommand::Services {
                command: ServicesCommand::Reload(ManagedServiceArgs {
                    service: ManagedServiceKind::Daemon,
                }),
            },
        )
        .await
        .expect("dispatch");
        assert_eq!(outcome.summary, "services.reload");

        let outcome = dispatch(
            &backend,
            &context,
            SkillCommand::Git {
                command: GitCommand::Branch {
                    command: GitBranchCommand::List(GitRepoArgs {
                        repo_root: None,
                        worktree_path: None,
                    }),
                },
            },
        )
        .await
        .expect("dispatch");
        assert_eq!(outcome.summary, "git.branch.list");

        let outcome = dispatch(
            &backend,
            &context,
            SkillCommand::I3 {
                command: I3Command::Workspace {
                    command: I3WorkspaceCommand::List(I3ListArgs::default()),
                },
            },
        )
        .await
        .expect("dispatch");
        assert_eq!(outcome.summary, "i3.workspace.list");

        assert_eq!(
            backend.calls.lock().expect("lock call log").as_slice(),
            [
                "process.signal",
                "services.reload",
                "git.branch.list",
                "i3.workspace.list"
            ]
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct StubBackend {
        calls: Arc<Mutex<Vec<String>>>,
    }

    impl StubBackend {
        fn push(&self, call: impl Into<String>) {
            self.calls.lock().expect("lock call log").push(call.into());
        }
    }

    #[async_trait(?Send)]
    impl SkillBackend for StubBackend {
        async fn agent_spawn(&self, _: &SkillContext, _: &AgentSpawnArgs) -> Result<SkillOutcome> {
            self.push("agent.spawn");
            Ok(SkillOutcome::new("agent.spawn"))
        }

        async fn agent_inspect(
            &self,
            _: &SkillContext,
            _: &AgentInspectArgs,
        ) -> Result<SkillOutcome> {
            self.push("agent.inspect");
            Ok(SkillOutcome::new("agent.inspect"))
        }

        async fn agent_resume(&self, _: &SkillContext, _: &ResumeArgs) -> Result<SkillOutcome> {
            self.push("agent.resume");
            Ok(SkillOutcome::new("agent.resume"))
        }

        async fn agent_retire(
            &self,
            _: &SkillContext,
            _: &AgentRetireArgs,
        ) -> Result<SkillOutcome> {
            self.push("agent.retire");
            Ok(SkillOutcome::new("agent.retire"))
        }

        async fn i3_status(&self, _: &SkillContext, _: &I3StatusArgs) -> Result<SkillOutcome> {
            self.push("i3.status");
            Ok(SkillOutcome::new("i3.status"))
        }

        async fn i3_attach(&self, _: &SkillContext, _: &I3AttachArgs) -> Result<SkillOutcome> {
            self.push("i3.attach");
            Ok(SkillOutcome::new("i3.attach"))
        }

        async fn i3_focus(&self, _: &SkillContext, _: &I3FocusArgs) -> Result<SkillOutcome> {
            self.push("i3.focus");
            Ok(SkillOutcome::new("i3.focus"))
        }

        async fn tt_status(&self, _: &SkillContext, _: &TTStatusArgs) -> Result<SkillOutcome> {
            self.push("tt.status");
            Ok(SkillOutcome::new("tt.status"))
        }

        async fn tt_spawn(&self, _: &SkillContext, _: &TTSpawnArgs) -> Result<SkillOutcome> {
            self.push("tt.spawn");
            Ok(SkillOutcome::new("tt.spawn"))
        }

        async fn tt_resume(&self, _: &SkillContext, _: &ResumeArgs) -> Result<SkillOutcome> {
            self.push("tt.resume");
            Ok(SkillOutcome::new("tt.resume"))
        }

        async fn tt_app_server_status(
            &self,
            _: &SkillContext,
            _: &AppServerNameArgs,
        ) -> Result<SkillOutcome> {
            self.push("tt.app_server.status");
            Ok(SkillOutcome::new("tt.app_server.status"))
        }

        async fn tt_app_server_start(
            &self,
            _: &SkillContext,
            _: &AppServerNameArgs,
        ) -> Result<SkillOutcome> {
            self.push("tt.app_server.start");
            Ok(SkillOutcome::new("tt.app_server.start"))
        }

        async fn tt_app_server_stop(
            &self,
            _: &SkillContext,
            _: &AppServerNameArgs,
        ) -> Result<SkillOutcome> {
            self.push("tt.app_server.stop");
            Ok(SkillOutcome::new("tt.app_server.stop"))
        }

        async fn tt_app_server_restart(
            &self,
            _: &SkillContext,
            _: &AppServerNameArgs,
        ) -> Result<SkillOutcome> {
            self.push("tt.app_server.restart");
            Ok(SkillOutcome::new("tt.app_server.restart"))
        }

        async fn process_status(
            &self,
            _: &SkillContext,
            _: &ProcessTargetArgs,
        ) -> Result<SkillOutcome> {
            self.push("process.status");
            Ok(SkillOutcome::new("process.status"))
        }

        async fn process_inspect(
            &self,
            _: &SkillContext,
            _: &ProcessTargetArgs,
        ) -> Result<SkillOutcome> {
            self.push("process.inspect");
            Ok(SkillOutcome::new("process.inspect"))
        }

        async fn process_start(
            &self,
            _: &SkillContext,
            _: &ProcessStartArgs,
        ) -> Result<SkillOutcome> {
            self.push("process.start");
            Ok(SkillOutcome::new("process.start"))
        }

        async fn process_stop(
            &self,
            _: &SkillContext,
            _: &ProcessTargetArgs,
        ) -> Result<SkillOutcome> {
            self.push("process.stop");
            Ok(SkillOutcome::new("process.stop"))
        }

        async fn process_restart(
            &self,
            _: &SkillContext,
            _: &ProcessStartArgs,
        ) -> Result<SkillOutcome> {
            self.push("process.restart");
            Ok(SkillOutcome::new("process.restart"))
        }

        async fn process_signal(
            &self,
            _: &SkillContext,
            _: &ProcessSignalArgs,
        ) -> Result<SkillOutcome> {
            self.push("process.signal");
            Ok(SkillOutcome::new("process.signal"))
        }

        async fn process_tree(
            &self,
            _: &SkillContext,
            _: &ProcessTargetArgs,
        ) -> Result<SkillOutcome> {
            self.push("process.tree");
            Ok(SkillOutcome::new("process.tree"))
        }

        async fn services_status(
            &self,
            _: &SkillContext,
            _: &ManagedServiceArgs,
        ) -> Result<SkillOutcome> {
            self.push("services.status");
            Ok(SkillOutcome::new("services.status"))
        }

        async fn services_inspect(
            &self,
            _: &SkillContext,
            _: &ManagedServiceArgs,
        ) -> Result<SkillOutcome> {
            self.push("services.inspect");
            Ok(SkillOutcome::new("services.inspect"))
        }

        async fn services_start(
            &self,
            _: &SkillContext,
            _: &ManagedServiceArgs,
        ) -> Result<SkillOutcome> {
            self.push("services.start");
            Ok(SkillOutcome::new("services.start"))
        }

        async fn services_stop(
            &self,
            _: &SkillContext,
            _: &ManagedServiceArgs,
        ) -> Result<SkillOutcome> {
            self.push("services.stop");
            Ok(SkillOutcome::new("services.stop"))
        }

        async fn services_restart(
            &self,
            _: &SkillContext,
            _: &ManagedServiceArgs,
        ) -> Result<SkillOutcome> {
            self.push("services.restart");
            Ok(SkillOutcome::new("services.restart"))
        }

        async fn services_reload(
            &self,
            _: &SkillContext,
            _: &ManagedServiceArgs,
        ) -> Result<SkillOutcome> {
            self.push("services.reload");
            Ok(SkillOutcome::new("services.reload"))
        }

        async fn git_status(&self, _: &SkillContext, _: &GitRepoArgs) -> Result<SkillOutcome> {
            self.push("git.status");
            Ok(SkillOutcome::new("git.status"))
        }

        async fn git_branch_current(
            &self,
            _: &SkillContext,
            _: &GitRepoArgs,
        ) -> Result<SkillOutcome> {
            self.push("git.branch.current");
            Ok(SkillOutcome::new("git.branch.current"))
        }

        async fn git_branch_list(&self, _: &SkillContext, _: &GitRepoArgs) -> Result<SkillOutcome> {
            self.push("git.branch.list");
            Ok(SkillOutcome::new("git.branch.list"))
        }

        async fn git_worktree_current(
            &self,
            _: &SkillContext,
            _: &GitRepoArgs,
        ) -> Result<SkillOutcome> {
            self.push("git.worktree.current");
            Ok(SkillOutcome::new("git.worktree.current"))
        }

        async fn git_worktree_list(
            &self,
            _: &SkillContext,
            _: &GitRepoArgs,
        ) -> Result<SkillOutcome> {
            self.push("git.worktree.list");
            Ok(SkillOutcome::new("git.worktree.list"))
        }

        async fn i3_window_info(&self, _: &SkillContext, _: &I3WindowArgs) -> Result<SkillOutcome> {
            self.push("i3.window.info");
            Ok(SkillOutcome::new("i3.window.info"))
        }

        async fn i3_workspace_list(
            &self,
            _: &SkillContext,
            _: &I3ListArgs,
        ) -> Result<SkillOutcome> {
            self.push("i3.workspace.list");
            Ok(SkillOutcome::new("i3.workspace.list"))
        }

        async fn i3_workspace_focus(
            &self,
            _: &SkillContext,
            _: &I3WorkspaceArgs,
        ) -> Result<SkillOutcome> {
            self.push("i3.workspace.focus");
            Ok(SkillOutcome::new("i3.workspace.focus"))
        }

        async fn i3_workspace_move(
            &self,
            _: &SkillContext,
            _: &I3WorkspaceArgs,
        ) -> Result<SkillOutcome> {
            self.push("i3.workspace.move");
            Ok(SkillOutcome::new("i3.workspace.move"))
        }

        async fn i3_window_focus(
            &self,
            _: &SkillContext,
            _: &I3WindowArgs,
        ) -> Result<SkillOutcome> {
            self.push("i3.window.focus");
            Ok(SkillOutcome::new("i3.window.focus"))
        }

        async fn i3_window_move(
            &self,
            _: &SkillContext,
            _: &I3WindowMoveArgs,
        ) -> Result<SkillOutcome> {
            self.push("i3.window.move");
            Ok(SkillOutcome::new("i3.window.move"))
        }

        async fn i3_window_close(
            &self,
            _: &SkillContext,
            _: &I3WindowArgs,
        ) -> Result<SkillOutcome> {
            self.push("i3.window.close");
            Ok(SkillOutcome::new("i3.window.close"))
        }

        async fn i3_message(&self, _: &SkillContext, _: &I3MessageArgs) -> Result<SkillOutcome> {
            self.push("i3.message");
            Ok(SkillOutcome::new("i3.message"))
        }

        async fn skill_apply(
            &self,
            _: &SkillContext,
            _: &SkillApplyArgs,
        ) -> Result<SkillOutcome> {
            self.push("skill.apply");
            Ok(SkillOutcome::new("skill.apply"))
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn dispatch_routes_skill_commands() {
        let backend = StubBackend::default();
        let context = SkillContext::default();
        let outcome = dispatch(
            &backend,
            &context,
            SkillCommand::TT {
                command: TTCommand::AppServer {
                    command: AppServerCommand::Restart(AppServerNameArgs {
                        name: "default".to_string(),
                    }),
                },
            },
        )
        .await
        .expect("dispatch");
        assert_eq!(outcome.summary, "tt.app_server.restart");
        assert_eq!(
            backend.calls.lock().expect("lock call log").as_slice(),
            ["tt.app_server.restart"]
        );

        let outcome = dispatch(
            &backend,
            &context,
            SkillCommand::Git {
                command: GitCommand::Worktree {
                    command: GitWorktreeCommand::List(GitRepoArgs {
                        repo_root: None,
                        worktree_path: None,
                    }),
                },
            },
        )
        .await
        .expect("dispatch");
        assert_eq!(outcome.summary, "git.worktree.list");
        assert_eq!(
            backend.calls.lock().expect("lock call log").as_slice(),
            ["tt.app_server.restart", "git.worktree.list"]
        );

        let outcome = dispatch(
            &backend,
            &context,
            SkillCommand::Apply(SkillApplyArgs {
                snapshot_id: "snapshot-1".to_string(),
                skills: vec!["chat".to_string()],
                out: None,
            }),
        )
        .await
        .expect("dispatch");
        assert_eq!(outcome.summary, "skill.apply");
        assert_eq!(
            backend.calls.lock().expect("lock call log").as_slice(),
            ["tt.app_server.restart", "git.worktree.list", "skill.apply"]
        );
    }
}
