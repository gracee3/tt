use std::env;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Result, anyhow, bail};
use async_trait::async_trait;

use super::service::{RuntimeOverrides, SupervisorService};
use tt_skills::{
    AgentInspectArgs, AgentRetireArgs, AppServerNameArgs, CodexStatusArgs, GitRepoArgs,
    I3AttachArgs, I3ListArgs, I3MessageArgs, I3StatusArgs, I3WindowArgs, I3WindowMoveArgs,
    I3WorkspaceArgs, ManagedServiceArgs, ManagedServiceKind, ProcessSignalArgs, ProcessStartArgs,
    ProcessTargetArgs, ResumeArgs, SkillBackend, SkillContext, SkillOutcome,
};

#[derive(Debug, Clone)]
pub struct OrcasSkillBackend {
    overrides: RuntimeOverrides,
}

impl OrcasSkillBackend {
    pub fn new(overrides: RuntimeOverrides) -> Self {
        Self { overrides }
    }

    fn service_overrides(&self, context: &SkillContext) -> RuntimeOverrides {
        let mut overrides = self.overrides.clone();
        overrides.codex_bin = context.codex_bin.clone();
        overrides.listen_url = context.listen_url.clone();
        overrides.inbox_mirror_server_url = context.inbox_mirror_server_url.clone();
        overrides.cwd = context.cwd.clone();
        overrides.worktree_root = context.worktree_root.clone();
        overrides.model = context.model.clone();
        overrides.connect_only = context.connect_only;
        overrides.force_spawn = context.force_spawn;
        overrides
    }

    async fn service(&self, context: &SkillContext) -> Result<SupervisorService> {
        SupervisorService::load(&self.service_overrides(context)).await
    }

    fn outcome(summary: impl Into<String>) -> SkillOutcome {
        SkillOutcome::new(summary)
    }

    fn current_dir(context: &SkillContext) -> Result<PathBuf> {
        context
            .cwd
            .clone()
            .or_else(|| env::current_dir().ok())
            .ok_or_else(|| anyhow!("unable to determine current working directory"))
    }

    fn repo_root(context: &SkillContext, args: &GitRepoArgs) -> Result<PathBuf> {
        args.repo_root
            .clone()
            .or_else(|| context.cwd.clone())
            .or_else(|| env::current_dir().ok())
            .ok_or_else(|| anyhow!("unable to determine repository root"))
    }

    fn run_capture(command: &str, args: &[&str], cwd: Option<&Path>) -> Result<String> {
        let mut process = Command::new(command);
        process.args(args);
        if let Some(cwd) = cwd {
            process.current_dir(cwd);
        }
        let output = process
            .output()
            .map_err(|error| anyhow!("failed to run `{command}`: {error}"))?;
        if !output.status.success() {
            bail!(
                "`{command}` exited with status {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr)
            );
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    fn run_wm(command: &str, args: &[&str]) -> Result<String> {
        let output = Command::new(command)
            .args(args)
            .output()
            .map_err(|error| anyhow!("failed to run `{command}`: {error}"))?;
        if !output.status.success() {
            bail!(
                "`{command}` exited with status {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr)
            );
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    fn run_process_command(command: &str, args: &[String], cwd: Option<&Path>) -> Result<u32> {
        let mut process = Command::new(command);
        process.args(args);
        if let Some(cwd) = cwd {
            process.current_dir(cwd);
        }
        let child = process
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| anyhow!("failed to spawn `{command}`: {error}"))?;
        Ok(child.id())
    }

    fn service_name(service: &ManagedServiceKind) -> &'static str {
        match service {
            ManagedServiceKind::Daemon => "daemon",
            ManagedServiceKind::AppServer => "app-server",
        }
    }

    fn selected_wm() -> Option<(&'static str, &'static str)> {
        if env::var_os("SWAYSOCK").is_some() || env::var_os("WAYLAND_DISPLAY").is_some() {
            Some(("swaymsg", "wayland"))
        } else if env::var_os("DISPLAY").is_some() {
            Some(("i3-msg", "x11"))
        } else {
            None
        }
    }

    fn window_manager_status() -> Result<(&'static str, &'static str)> {
        if let Some(selected) = Self::selected_wm() {
            return Ok(selected);
        }
        bail!("no i3 or sway session discovered");
    }

    fn process_target_label(args: &ProcessTargetArgs) -> Result<String> {
        if let Some(pid) = args.pid {
            return Ok(format!("pid {pid}"));
        }
        if let Some(name) = args.name.as_deref() {
            return Ok(format!("name {name}"));
        }
        bail!("either --pid or --name is required")
    }
}

#[async_trait(?Send)]
impl SkillBackend for OrcasSkillBackend {
    async fn agent_spawn(
        &self,
        context: &SkillContext,
        args: &tt_skills::AgentSpawnArgs,
    ) -> Result<SkillOutcome> {
        let service = self.service(context).await?;
        service
            .codex_spawn(
                &args.role,
                args.workstream.as_deref(),
                args.new_workstream.as_deref(),
                args.repo_root.clone(),
                args.headless,
                args.model.clone(),
            )
            .await?;
        Ok(Self::outcome("agent.spawn"))
    }

    async fn agent_inspect(
        &self,
        context: &SkillContext,
        args: &AgentInspectArgs,
    ) -> Result<SkillOutcome> {
        let service = self.service(context).await?;
        if let Some(thread) = args.thread.as_deref() {
            service.thread_read(thread).await?;
        } else if let Some(workstream) = args.workstream.as_deref() {
            service.threads_list_loaded(workstream).await?;
        } else if let Err(error) = service.session_active().await {
            println!("session_status_error: {error}");
        }
        Ok(Self::outcome("agent.inspect"))
    }

    async fn agent_resume(
        &self,
        context: &SkillContext,
        args: &ResumeArgs,
    ) -> Result<SkillOutcome> {
        let service = self.service(context).await?;
        service
            .thread_resume(&args.thread, args.cwd.clone(), args.model.clone())
            .await?;
        Ok(Self::outcome("agent.resume"))
    }

    async fn agent_retire(
        &self,
        context: &SkillContext,
        args: &AgentRetireArgs,
    ) -> Result<SkillOutcome> {
        let service = self.service(context).await?;
        service.thread_read(&args.thread).await?;
        println!("retire_note: {}", args.note.as_deref().unwrap_or("unset"));
        println!("retire_status: advisory-only");
        Ok(Self::outcome("agent.retire"))
    }

    async fn i3_status(&self, _: &SkillContext, _: &I3StatusArgs) -> Result<SkillOutcome> {
        let (command, session_kind) = Self::window_manager_status()?;
        let display = env::var("DISPLAY").ok();
        let wayland_display = env::var("WAYLAND_DISPLAY").ok();
        let swaysock = env::var("SWAYSOCK").ok();
        let workspaces = Self::run_wm(command, &["-t", "get_workspaces"])?;
        println!("wm_command: {command}");
        println!("session_kind: {session_kind}");
        println!("display: {}", display.as_deref().unwrap_or("-"));
        println!(
            "wayland_display: {}",
            wayland_display.as_deref().unwrap_or("-")
        );
        println!("swaysock: {}", swaysock.as_deref().unwrap_or("-"));
        println!("workspace_bytes: {}", workspaces.len());
        println!("status: reachable");
        Ok(Self::outcome("i3.status"))
    }

    async fn i3_attach(&self, _: &SkillContext, _: &I3AttachArgs) -> Result<SkillOutcome> {
        let (command, session_kind) = Self::window_manager_status()?;
        let tree = Self::run_wm(command, &["-t", "get_tree"])?;
        println!("wm_command: {command}");
        println!("session_kind: {session_kind}");
        println!("tree_bytes: {}", tree.len());
        println!("status: attached");
        Ok(Self::outcome("i3.attach"))
    }

    async fn i3_focus(
        &self,
        _: &SkillContext,
        args: &tt_skills::I3FocusArgs,
    ) -> Result<SkillOutcome> {
        let (command, session_kind) = Self::window_manager_status()?;
        let workspace = args.workspace.as_deref().unwrap_or("current");
        let _ = Self::run_wm(command, &["workspace", workspace])?;
        println!("wm_command: {command}");
        println!("session_kind: {session_kind}");
        println!("workspace: {workspace}");
        println!("status: focused");
        Ok(Self::outcome("i3.focus"))
    }

    async fn i3_workspace_focus(
        &self,
        _: &SkillContext,
        args: &I3WorkspaceArgs,
    ) -> Result<SkillOutcome> {
        let (command, session_kind) = Self::window_manager_status()?;
        let _ = Self::run_wm(command, &["workspace", args.workspace.as_str()])?;
        println!("wm_command: {command}");
        println!("session_kind: {session_kind}");
        println!("workspace: {}", args.workspace);
        println!("action: focus");
        Ok(Self::outcome("i3.workspace.focus"))
    }

    async fn i3_workspace_move(
        &self,
        _: &SkillContext,
        args: &I3WorkspaceArgs,
    ) -> Result<SkillOutcome> {
        let (command, session_kind) = Self::window_manager_status()?;
        let workspace = args.workspace.as_str();
        let _ = Self::run_wm(
            command,
            &["move", "container", "to", "workspace", workspace],
        )?;
        println!("wm_command: {command}");
        println!("session_kind: {session_kind}");
        println!("workspace: {workspace}");
        println!("action: move");
        Ok(Self::outcome("i3.workspace.move"))
    }

    async fn i3_window_focus(&self, _: &SkillContext, args: &I3WindowArgs) -> Result<SkillOutcome> {
        let (command, session_kind) = Self::window_manager_status()?;
        let criteria = format!("[{}]", args.criteria);
        let _ = Self::run_wm(command, &[&criteria, "focus"])?;
        println!("wm_command: {command}");
        println!("session_kind: {session_kind}");
        println!("criteria: {}", args.criteria);
        println!("action: focus");
        Ok(Self::outcome("i3.window.focus"))
    }

    async fn i3_window_move(
        &self,
        _: &SkillContext,
        args: &I3WindowMoveArgs,
    ) -> Result<SkillOutcome> {
        let (command, session_kind) = Self::window_manager_status()?;
        let criteria = format!("[{}]", args.criteria);
        let _ = Self::run_wm(
            command,
            &[
                &criteria,
                "move",
                "container",
                "to",
                "workspace",
                args.workspace.as_str(),
            ],
        )?;
        println!("wm_command: {command}");
        println!("session_kind: {session_kind}");
        println!("criteria: {}", args.criteria);
        println!("workspace: {}", args.workspace);
        println!("action: move");
        Ok(Self::outcome("i3.window.move"))
    }

    async fn i3_window_close(&self, _: &SkillContext, args: &I3WindowArgs) -> Result<SkillOutcome> {
        let (command, session_kind) = Self::window_manager_status()?;
        let criteria = format!("[{}]", args.criteria);
        let _ = Self::run_wm(command, &[&criteria, "kill"])?;
        println!("wm_command: {command}");
        println!("session_kind: {session_kind}");
        println!("criteria: {}", args.criteria);
        println!("action: close");
        Ok(Self::outcome("i3.window.close"))
    }

    async fn i3_message(&self, _: &SkillContext, args: &I3MessageArgs) -> Result<SkillOutcome> {
        let (command, session_kind) = Self::window_manager_status()?;
        if args.message.is_empty() {
            bail!("at least one raw i3/sway message argument is required");
        }
        let rendered = args.message.join(" ");
        let message_args = args.message.iter().map(String::as_str).collect::<Vec<_>>();
        let _ = Self::run_wm(command, &message_args)?;
        println!("wm_command: {command}");
        println!("session_kind: {session_kind}");
        println!("message: {rendered}");
        println!("action: raw");
        Ok(Self::outcome("i3.message"))
    }

    async fn codex_status(
        &self,
        context: &SkillContext,
        _: &CodexStatusArgs,
    ) -> Result<SkillOutcome> {
        let service = self.service(context).await?;
        let mut succeeded = false;
        match service.session_active().await {
            Ok(()) => succeeded = true,
            Err(error) => println!("session_status_error: {error}"),
        }
        match service.app_server_info("default").await {
            Ok(()) => succeeded = true,
            Err(error) => println!("app_server_status_error: {error}"),
        }
        if !succeeded {
            bail!("codex status could not reach the session or app-server runtime");
        }
        Ok(Self::outcome("codex.status"))
    }

    async fn codex_spawn(
        &self,
        context: &SkillContext,
        args: &tt_skills::CodexSpawnArgs,
    ) -> Result<SkillOutcome> {
        let service = self.service(context).await?;
        service
            .codex_spawn(
                &args.role,
                args.workstream.as_deref(),
                args.new_workstream.as_deref(),
                args.repo_root.clone(),
                args.headless,
                args.model.clone(),
            )
            .await?;
        Ok(Self::outcome("codex.spawn"))
    }

    async fn codex_resume(
        &self,
        context: &SkillContext,
        args: &ResumeArgs,
    ) -> Result<SkillOutcome> {
        let service = self.service(context).await?;
        service
            .thread_resume(&args.thread, args.cwd.clone(), args.model.clone())
            .await?;
        Ok(Self::outcome("codex.resume"))
    }

    async fn codex_app_server_status(
        &self,
        context: &SkillContext,
        args: &AppServerNameArgs,
    ) -> Result<SkillOutcome> {
        let service = self.service(context).await?;
        service.app_server_status(&args.name).await?;
        Ok(Self::outcome("codex.app_server.status"))
    }

    async fn codex_app_server_start(
        &self,
        context: &SkillContext,
        args: &AppServerNameArgs,
    ) -> Result<SkillOutcome> {
        let service = self.service(context).await?;
        service.app_server_start(&args.name).await?;
        Ok(Self::outcome("codex.app_server.start"))
    }

    async fn codex_app_server_stop(
        &self,
        context: &SkillContext,
        args: &AppServerNameArgs,
    ) -> Result<SkillOutcome> {
        let service = self.service(context).await?;
        service.app_server_stop(&args.name).await?;
        Ok(Self::outcome("codex.app_server.stop"))
    }

    async fn codex_app_server_restart(
        &self,
        context: &SkillContext,
        args: &AppServerNameArgs,
    ) -> Result<SkillOutcome> {
        let service = self.service(context).await?;
        service.app_server_restart(&args.name).await?;
        Ok(Self::outcome("codex.app_server.restart"))
    }

    async fn process_status(
        &self,
        context: &SkillContext,
        args: &ProcessTargetArgs,
    ) -> Result<SkillOutcome> {
        let label = Self::process_target_label(args)?;
        let output = if let Some(pid) = args.pid {
            Self::run_capture(
                "ps",
                &["-p", &pid.to_string(), "-o", "pid=,ppid=,etime=,command="],
                Some(Self::current_dir(context)?.as_path()),
            )?
        } else if let Some(name) = args.name.as_deref() {
            Self::run_capture(
                "pgrep",
                &["-af", name],
                Some(Self::current_dir(context)?.as_path()),
            )?
        } else {
            bail!("either --pid or --name is required");
        };
        println!("target: {label}");
        println!("status: running");
        println!("{output}");
        Ok(Self::outcome("process.status"))
    }

    async fn process_inspect(
        &self,
        context: &SkillContext,
        args: &ProcessTargetArgs,
    ) -> Result<SkillOutcome> {
        let label = Self::process_target_label(args)?;
        let output = if let Some(pid) = args.pid {
            Self::run_capture(
                "ps",
                &[
                    "-p",
                    &pid.to_string(),
                    "-o",
                    "pid=,ppid=,etime=,command=,args=",
                    "-ww",
                ],
                Some(Self::current_dir(context)?.as_path()),
            )?
        } else if let Some(name) = args.name.as_deref() {
            Self::run_capture(
                "pgrep",
                &["-af", name],
                Some(Self::current_dir(context)?.as_path()),
            )?
        } else {
            bail!("either --pid or --name is required");
        };
        println!("target: {label}");
        println!("{output}");
        Ok(Self::outcome("process.inspect"))
    }

    async fn process_start(
        &self,
        context: &SkillContext,
        args: &ProcessStartArgs,
    ) -> Result<SkillOutcome> {
        if args.command.is_empty() {
            bail!("process start requires a command to execute");
        }
        let cwd = args.cwd.as_deref().or(context.cwd.as_deref());
        let pid = Self::run_process_command(&args.command[0], &args.command[1..], cwd)?;
        println!("status: started");
        if let Some(name) = args.name.as_deref() {
            println!("name: {name}");
        }
        if let Some(pid_hint) = args.pid {
            println!("requested_pid: {pid_hint}");
        }
        println!("pid: {pid}");
        println!("command: {}", args.command.join(" "));
        Ok(Self::outcome("process.start"))
    }

    async fn process_stop(
        &self,
        _: &SkillContext,
        args: &ProcessTargetArgs,
    ) -> Result<SkillOutcome> {
        if let Some(pid) = args.pid {
            let status = Command::new("kill")
                .args(["-TERM", &pid.to_string()])
                .status()
                .map_err(|error| anyhow!("failed to run `kill -TERM {pid}`: {error}"))?;
            if !status.success() {
                bail!("`kill -TERM {pid}` failed with status {status}");
            }
            println!("status: stopped");
            println!("pid: {pid}");
        } else if let Some(name) = args.name.as_deref() {
            let status = Command::new("pkill")
                .args(["-TERM", "-f", name])
                .status()
                .map_err(|error| anyhow!("failed to run `pkill -TERM -f {name}`: {error}"))?;
            if !status.success() {
                bail!("`pkill -TERM -f {name}` failed with status {status}");
            }
            println!("status: stopped");
            println!("name: {name}");
        } else {
            bail!("either --pid or --name is required");
        }
        Ok(Self::outcome("process.stop"))
    }

    async fn process_restart(
        &self,
        context: &SkillContext,
        args: &ProcessStartArgs,
    ) -> Result<SkillOutcome> {
        if let Some(pid) = args.pid {
            let status = Command::new("kill")
                .args(["-TERM", &pid.to_string()])
                .status()
                .map_err(|error| anyhow!("failed to run `kill -TERM {pid}`: {error}"))?;
            if !status.success() {
                bail!("`kill -TERM {pid}` failed with status {status}");
            }
        } else if let Some(name) = args.name.as_deref() {
            let status = Command::new("pkill")
                .args(["-TERM", "-f", name])
                .status()
                .map_err(|error| anyhow!("failed to run `pkill -TERM -f {name}`: {error}"))?;
            if !status.success() {
                bail!("`pkill -TERM -f {name}` failed with status {status}");
            }
        }
        self.process_start(context, args).await
    }

    async fn process_signal(
        &self,
        _: &SkillContext,
        args: &ProcessSignalArgs,
    ) -> Result<SkillOutcome> {
        let label = Self::process_target_label(&ProcessTargetArgs {
            pid: args.pid,
            name: args.name.clone(),
        })?;
        if let Some(pid) = args.pid {
            let status = Command::new("kill")
                .args(["-s", &args.signal, &pid.to_string()])
                .status()
                .map_err(|error| {
                    anyhow!("failed to run `kill -s {} {pid}`: {error}", args.signal)
                })?;
            if !status.success() {
                bail!(
                    "`kill -s {} {pid}` failed with status {status}",
                    args.signal
                );
            }
        } else if let Some(name) = args.name.as_deref() {
            let status = Command::new("pkill")
                .args([
                    format!("-{}", args.signal),
                    "-f".to_string(),
                    name.to_string(),
                ])
                .status()
                .map_err(|error| {
                    anyhow!("failed to run `pkill -{} -f {name}`: {error}", args.signal)
                })?;
            if !status.success() {
                bail!(
                    "`pkill -{} -f {name}` failed with status {status}",
                    args.signal
                );
            }
        } else {
            bail!("either --pid or --name is required");
        }
        println!("target: {label}");
        println!("signal: {}", args.signal);
        println!("status: signaled");
        Ok(Self::outcome("process.signal"))
    }

    async fn process_tree(
        &self,
        context: &SkillContext,
        args: &ProcessTargetArgs,
    ) -> Result<SkillOutcome> {
        let label = Self::process_target_label(args)?;
        let output = if let Some(pid) = args.pid {
            Self::run_capture(
                "ps",
                &[
                    "-p",
                    &pid.to_string(),
                    "-o",
                    "pid=,ppid=,etime=,command=",
                    "--forest",
                ],
                Some(Self::current_dir(context)?.as_path()),
            )?
        } else if let Some(name) = args.name.as_deref() {
            Self::run_capture(
                "pgrep",
                &["-af", name],
                Some(Self::current_dir(context)?.as_path()),
            )?
        } else {
            bail!("either --pid or --name is required");
        };
        println!("target: {label}");
        println!("tree: available");
        println!("{output}");
        Ok(Self::outcome("process.tree"))
    }

    async fn services_status(
        &self,
        context: &SkillContext,
        args: &ManagedServiceArgs,
    ) -> Result<SkillOutcome> {
        let service = self.service(context).await?;
        match args.service {
            ManagedServiceKind::Daemon => service.daemon_status().await?,
            ManagedServiceKind::AppServer => service.app_server_status("default").await?,
        }
        println!("service: {}", Self::service_name(&args.service));
        Ok(Self::outcome("services.status"))
    }

    async fn services_inspect(
        &self,
        context: &SkillContext,
        args: &ManagedServiceArgs,
    ) -> Result<SkillOutcome> {
        let service = self.service(context).await?;
        match args.service {
            ManagedServiceKind::Daemon => service.daemon_status().await?,
            ManagedServiceKind::AppServer => service.app_server_info("default").await?,
        }
        println!("service: {}", Self::service_name(&args.service));
        println!("ownership: managed");
        Ok(Self::outcome("services.inspect"))
    }

    async fn services_start(
        &self,
        context: &SkillContext,
        args: &ManagedServiceArgs,
    ) -> Result<SkillOutcome> {
        let service = self.service(context).await?;
        match args.service {
            ManagedServiceKind::Daemon => service.daemon_start(context.force_spawn).await?,
            ManagedServiceKind::AppServer => service.app_server_start("default").await?,
        }
        Ok(Self::outcome("services.start"))
    }

    async fn services_stop(
        &self,
        context: &SkillContext,
        args: &ManagedServiceArgs,
    ) -> Result<SkillOutcome> {
        let service = self.service(context).await?;
        match args.service {
            ManagedServiceKind::Daemon => service.daemon_stop().await?,
            ManagedServiceKind::AppServer => service.app_server_stop("default").await?,
        }
        Ok(Self::outcome("services.stop"))
    }

    async fn services_restart(
        &self,
        context: &SkillContext,
        args: &ManagedServiceArgs,
    ) -> Result<SkillOutcome> {
        let service = self.service(context).await?;
        match args.service {
            ManagedServiceKind::Daemon => service.daemon_restart().await?,
            ManagedServiceKind::AppServer => service.app_server_restart("default").await?,
        }
        Ok(Self::outcome("services.restart"))
    }

    async fn services_reload(
        &self,
        context: &SkillContext,
        args: &ManagedServiceArgs,
    ) -> Result<SkillOutcome> {
        self.services_restart(context, args).await
    }

    async fn git_status(&self, context: &SkillContext, args: &GitRepoArgs) -> Result<SkillOutcome> {
        let repo_root = Self::repo_root(context, args)?;
        let output = Self::run_capture(
            "git",
            &[
                "-C",
                repo_root.to_str().unwrap_or("."),
                "status",
                "--short",
                "--branch",
            ],
            None,
        )?;
        println!("repo_root: {}", repo_root.display());
        println!("{output}");
        Ok(Self::outcome("git.status"))
    }

    async fn git_branch_current(
        &self,
        context: &SkillContext,
        args: &GitRepoArgs,
    ) -> Result<SkillOutcome> {
        let repo_root = Self::repo_root(context, args)?;
        let output = Self::run_capture(
            "git",
            &[
                "-C",
                repo_root.to_str().unwrap_or("."),
                "branch",
                "--show-current",
            ],
            None,
        )?;
        println!("repo_root: {}", repo_root.display());
        println!("branch: {output}");
        Ok(Self::outcome("git.branch.current"))
    }

    async fn git_branch_list(
        &self,
        context: &SkillContext,
        args: &GitRepoArgs,
    ) -> Result<SkillOutcome> {
        let repo_root = Self::repo_root(context, args)?;
        let output = Self::run_capture(
            "git",
            &["-C", repo_root.to_str().unwrap_or("."), "branch", "--all"],
            None,
        )?;
        println!("repo_root: {}", repo_root.display());
        println!("{output}");
        Ok(Self::outcome("git.branch.list"))
    }

    async fn git_worktree_current(
        &self,
        context: &SkillContext,
        args: &GitRepoArgs,
    ) -> Result<SkillOutcome> {
        let repo_root = Self::repo_root(context, args)?;
        let current_branch = Self::run_capture(
            "git",
            &[
                "-C",
                repo_root.to_str().unwrap_or("."),
                "branch",
                "--show-current",
            ],
            None,
        )?;
        let current_worktree = Self::run_capture(
            "git",
            &[
                "-C",
                repo_root.to_str().unwrap_or("."),
                "rev-parse",
                "--show-toplevel",
            ],
            None,
        )?;
        println!("repo_root: {}", repo_root.display());
        println!("worktree_path: {current_worktree}");
        println!("branch: {current_branch}");
        Ok(Self::outcome("git.worktree.current"))
    }

    async fn git_worktree_list(
        &self,
        context: &SkillContext,
        args: &GitRepoArgs,
    ) -> Result<SkillOutcome> {
        let repo_root = Self::repo_root(context, args)?;
        let output = Self::run_capture(
            "git",
            &[
                "-C",
                repo_root.to_str().unwrap_or("."),
                "worktree",
                "list",
                "--porcelain",
            ],
            None,
        )?;
        println!("repo_root: {}", repo_root.display());
        if let Some(worktree_path) = args.worktree_path.as_ref() {
            println!("worktree_path: {}", worktree_path.display());
        }
        println!("{output}");
        Ok(Self::outcome("git.worktree.list"))
    }

    async fn i3_window_info(&self, _: &SkillContext, args: &I3WindowArgs) -> Result<SkillOutcome> {
        let (command, session_kind) = Self::window_manager_status()?;
        let tree = Self::run_wm(command, &["-t", "get_tree"])?;
        println!("wm_command: {command}");
        println!("session_kind: {session_kind}");
        println!("criteria: {}", args.criteria);
        println!("tree_bytes: {}", tree.len());
        println!("action: info");
        Ok(Self::outcome("i3.window.info"))
    }

    async fn i3_workspace_list(&self, _: &SkillContext, _: &I3ListArgs) -> Result<SkillOutcome> {
        let (command, session_kind) = Self::window_manager_status()?;
        let workspaces = Self::run_wm(command, &["-t", "get_workspaces"])?;
        println!("wm_command: {command}");
        println!("session_kind: {session_kind}");
        println!("action: list");
        println!("workspace_bytes: {}", workspaces.len());
        Ok(Self::outcome("i3.workspace.list"))
    }
}
