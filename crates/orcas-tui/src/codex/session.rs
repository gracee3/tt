use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow};
use crossterm::terminal;
#[cfg(unix)]
use nix::errno::Errno;
#[cfg(unix)]
use nix::fcntl::{FcntlArg, OFlag, fcntl};
#[cfg(unix)]
use nix::unistd::read as nix_read;
use portable_pty::{
    Child, ChildKiller, CommandBuilder, ExitStatus as PtyExitStatus, MasterPty, PtySize,
    native_pty_system,
};
use tracing::info;

use crate::app::AppState;

use super::preview::{CodexOutputPreview, render_preview_from_pty_bytes};
use super::ring_buffer::PtyRingBuffer;
use super::terminal::{OrcasTerminal, enter_pass_through_mode, suspend_terminal};

const RELAY_POLL_INTERVAL: Duration = Duration::from_millis(50);
const INPUT_IDLE_SLEEP: Duration = Duration::from_millis(10);
const DETACH_PREFIX: u8 = 0x1d;
const DETACH_SUFFIX: u8 = b'd';
const DETACH_SEQUENCE_TIMEOUT: Duration = Duration::from_millis(750);
const MAX_SESSION_HISTORY_PER_THREAD: usize = 4;
const SESSION_PREVIEW_LINES: usize = 3;
const SESSION_PREVIEW_WIDTH: usize = 84;

pub const DEFAULT_PTY_RING_BUFFER_CAPACITY: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CodexSessionId(u64);

impl CodexSessionId {
    #[must_use]
    pub fn as_u64(self) -> u64 {
        self.0
    }
}

impl From<u64> for CodexSessionId {
    fn from(value: u64) -> Self {
        Self(value)
    }
}

impl fmt::Display for CodexSessionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexThreadSessionSummary {
    pub session_id: CodexSessionId,
    pub thread_id: String,
    pub state: CodexSessionState,
    pub created_at: Instant,
    pub last_activity_at: Option<Instant>,
    pub output_preview: CodexOutputPreview,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CodexThreadSessions {
    pub thread_id: String,
    pub sessions: Vec<CodexThreadSessionSummary>,
}

impl CodexThreadSessions {
    #[must_use]
    pub fn current(&self) -> Option<&CodexThreadSessionSummary> {
        self.sessions.first()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexResumeDescriptor {
    pub thread_id: String,
    pub program: PathBuf,
    pub args: Vec<String>,
    pub cwd: Option<PathBuf>,
    pub env_overrides: BTreeMap<String, String>,
    pub display_command: String,
}

impl CodexResumeDescriptor {
    pub fn for_selected_thread(state: &AppState) -> Result<Self, CodexResumeDescriptorError> {
        let daemon = state
            .daemon
            .as_ref()
            .ok_or(CodexResumeDescriptorError::DaemonUnavailable)?;
        let thread_id = state
            .selected_thread_id
            .clone()
            .ok_or(CodexResumeDescriptorError::NoThreadSelected)?;
        let thread = selected_thread_summary(state, &thread_id)
            .ok_or_else(|| CodexResumeDescriptorError::UnknownThread(thread_id.clone()))?;
        let program = PathBuf::from(daemon.codex_binary_path.clone());
        if daemon.codex_binary_path.trim().is_empty() {
            return Err(CodexResumeDescriptorError::MissingCodexBinaryPath);
        }

        let cwd = if thread.cwd.trim().is_empty() {
            None
        } else {
            Some(PathBuf::from(thread.cwd.clone()))
        };

        let mut args = vec!["resume".to_string()];
        if let Some(cwd) = cwd.as_ref() {
            args.push("--cd".to_string());
            args.push(cwd.display().to_string());
        }
        args.push(thread_id.clone());

        let env_overrides = BTreeMap::new();
        let display_command = render_display_command(&program, &args, &env_overrides);

        Ok(Self {
            thread_id,
            program,
            args,
            cwd,
            env_overrides,
            display_command,
        })
    }

    #[must_use]
    pub fn pty_command_builder(&self) -> CommandBuilder {
        let mut command = CommandBuilder::new(&self.program);
        command.args(&self.args);
        if let Some(cwd) = self.cwd.as_ref() {
            command.cwd(cwd);
        }
        for (key, value) in &self.env_overrides {
            command.env(key, value);
        }
        command
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodexResumeDescriptorError {
    DaemonUnavailable,
    NoThreadSelected,
    UnknownThread(String),
    MissingCodexBinaryPath,
}

impl fmt::Display for CodexResumeDescriptorError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DaemonUnavailable => write!(formatter, "daemon status is not loaded yet"),
            Self::NoThreadSelected => write!(formatter, "no thread is selected"),
            Self::UnknownThread(thread_id) => {
                write!(formatter, "selected thread `{thread_id}` is not loaded")
            }
            Self::MissingCodexBinaryPath => {
                write!(formatter, "daemon did not provide a Codex binary path")
            }
        }
    }
}

impl std::error::Error for CodexResumeDescriptorError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodexSessionState {
    Launching,
    Attached { pid: u32 },
    Detached { pid: u32 },
    Exited { result: CodexExit },
    Failed { error: String },
}

impl CodexSessionState {
    #[must_use]
    pub fn is_live(&self) -> bool {
        matches!(
            self,
            Self::Launching | Self::Attached { .. } | Self::Detached { .. }
        )
    }

    #[must_use]
    pub fn pid(&self) -> Option<u32> {
        match self {
            Self::Attached { pid } | Self::Detached { pid } => Some(*pid),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexExit {
    pub success: bool,
    pub code: Option<i32>,
}

impl From<std::process::ExitStatus> for CodexExit {
    fn from(status: std::process::ExitStatus) -> Self {
        Self {
            success: status.success(),
            code: status.code(),
        }
    }
}

impl From<PtyExitStatus> for CodexExit {
    fn from(status: PtyExitStatus) -> Self {
        Self {
            success: status.success(),
            code: i32::try_from(status.exit_code()).ok(),
        }
    }
}

#[derive(Debug)]
pub struct CodexSession {
    pub id: CodexSessionId,
    pub thread_id: String,
    pub descriptor: CodexResumeDescriptor,
    pub state: CodexSessionState,
    pub pty_output: PtyRingBuffer,
    pub created_at: Instant,
    pub last_activity_at: Option<Instant>,
}

impl CodexSession {
    #[must_use]
    pub fn new(
        id: CodexSessionId,
        descriptor: CodexResumeDescriptor,
        ring_capacity: usize,
    ) -> Self {
        Self {
            id,
            thread_id: descriptor.thread_id.clone(),
            descriptor,
            state: CodexSessionState::Launching,
            pty_output: PtyRingBuffer::new(ring_capacity),
            created_at: Instant::now(),
            last_activity_at: None,
        }
    }

    pub fn mark_attached(&mut self, pid: u32) -> Result<(), SessionTransitionError> {
        match self.state {
            CodexSessionState::Launching | CodexSessionState::Detached { .. } => {
                self.state = CodexSessionState::Attached { pid };
                self.last_activity_at = Some(Instant::now());
                Ok(())
            }
            _ => Err(SessionTransitionError(
                "session can only attach from launching or detached",
            )),
        }
    }

    pub fn mark_detached(&mut self) -> Result<(), SessionTransitionError> {
        match self.state {
            CodexSessionState::Attached { pid } => {
                self.state = CodexSessionState::Detached { pid };
                Ok(())
            }
            _ => Err(SessionTransitionError(
                "session can only detach from attached",
            )),
        }
    }

    pub fn record_pty_output(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        self.pty_output.push(bytes);
        self.last_activity_at = Some(Instant::now());
    }

    pub fn mark_exited(&mut self, result: CodexExit) -> Result<(), SessionTransitionError> {
        match self.state {
            CodexSessionState::Launching
            | CodexSessionState::Attached { .. }
            | CodexSessionState::Detached { .. } => {
                self.state = CodexSessionState::Exited { result };
                Ok(())
            }
            _ => Err(SessionTransitionError(
                "session cannot exit from terminal state",
            )),
        }
    }

    pub fn mark_failed(&mut self, error: impl Into<String>) -> Result<(), SessionTransitionError> {
        match self.state {
            CodexSessionState::Exited { .. } | CodexSessionState::Failed { .. } => Err(
                SessionTransitionError("session cannot fail from terminal state"),
            ),
            _ => {
                self.state = CodexSessionState::Failed {
                    error: error.into(),
                };
                Ok(())
            }
        }
    }

    #[must_use]
    pub fn terminal_message(&self) -> Option<String> {
        match &self.state {
            CodexSessionState::Detached { .. } => Some(format!(
                "Detached Codex session for thread {}. Press c to reattach.",
                self.thread_id
            )),
            CodexSessionState::Exited { result } if !result.success => Some(match result.code {
                Some(code) => format!(
                    "Codex resume for thread {} exited with status {}.",
                    self.thread_id, code
                ),
                None => format!(
                    "Codex resume for thread {} exited unsuccessfully.",
                    self.thread_id
                ),
            }),
            CodexSessionState::Failed { error } => Some(format!(
                "Codex resume for thread {} failed: {}",
                self.thread_id, error
            )),
            _ => None,
        }
    }

    #[must_use]
    pub fn summary(&self) -> CodexThreadSessionSummary {
        CodexThreadSessionSummary {
            session_id: self.id,
            thread_id: self.thread_id.clone(),
            state: self.state.clone(),
            created_at: self.created_at,
            last_activity_at: self.last_activity_at,
            output_preview: render_preview_from_pty_bytes(
                &self.pty_output.snapshot(),
                SESSION_PREVIEW_LINES,
                SESSION_PREVIEW_WIDTH,
            ),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionTransitionError(&'static str);

impl fmt::Display for SessionTransitionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}

impl std::error::Error for SessionTransitionError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InputRelayControl {
    DetachRequested,
}

enum BackgroundEvent {
    Output {
        session_id: CodexSessionId,
        bytes: Vec<u8>,
    },
    Exited {
        session_id: CodexSessionId,
        result: CodexExit,
    },
    Failed {
        session_id: CodexSessionId,
        error: String,
    },
}

struct CodexSessionHost {
    pid: u32,
    master: Box<dyn MasterPty + Send>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    killer: Box<dyn ChildKiller + Send + Sync>,
}

pub struct CodexSessionManager {
    next_session_id: u64,
    ring_capacity: usize,
    sessions: BTreeMap<CodexSessionId, CodexSession>,
    hosts: BTreeMap<CodexSessionId, CodexSessionHost>,
    event_tx: mpsc::Sender<BackgroundEvent>,
    event_rx: mpsc::Receiver<BackgroundEvent>,
}

impl CodexSessionManager {
    #[must_use]
    pub fn new(ring_capacity: usize) -> Self {
        let (event_tx, event_rx) = mpsc::channel();
        Self {
            next_session_id: 1,
            ring_capacity,
            sessions: BTreeMap::new(),
            hosts: BTreeMap::new(),
            event_tx,
            event_rx,
        }
    }

    #[must_use]
    pub fn session(&self, session_id: CodexSessionId) -> Option<&CodexSession> {
        self.sessions.get(&session_id)
    }

    #[must_use]
    pub fn session_for_thread(&self, thread_id: &str) -> Option<&CodexSession> {
        self.live_session_id_for_thread(thread_id)
            .and_then(|session_id| self.sessions.get(&session_id))
            .or_else(|| {
                self.sessions
                    .values()
                    .rev()
                    .find(|session| session.thread_id == thread_id)
            })
    }

    pub fn thread_sessions(&self) -> HashMap<String, CodexThreadSessions> {
        let mut histories = HashMap::new();
        for session in self.sessions.values().rev() {
            let entry = histories
                .entry(session.thread_id.clone())
                .or_insert_with(|| CodexThreadSessions {
                    thread_id: session.thread_id.clone(),
                    sessions: Vec::new(),
                });
            if entry.sessions.len() < MAX_SESSION_HISTORY_PER_THREAD {
                entry.sessions.push(session.summary());
            }
        }
        histories
    }

    pub fn drain_background_events(&mut self) -> Result<bool> {
        let mut changed = false;
        while let Ok(event) = self.event_rx.try_recv() {
            changed |= self.handle_background_event(event, None)?;
        }
        Ok(changed)
    }

    pub fn attach_or_resume(
        &mut self,
        terminal: &mut OrcasTerminal,
        descriptor: CodexResumeDescriptor,
    ) -> Result<CodexSessionId> {
        self.drain_background_events()?;
        let session_id = match self.live_session_id_for_thread(&descriptor.thread_id) {
            Some(existing_id) => existing_id,
            None => {
                let new_id = self.insert_session(descriptor);
                self.spawn_pty_host(new_id)?;
                new_id
            }
        };

        let suspended = suspend_terminal(terminal)?;
        let run_result = self.run_suspended_attached_session(session_id);
        let restore_result = suspended.resume();
        let drain_result = self.drain_background_events();
        let cleanup_result = self.cleanup_terminal_sessions();

        match (run_result, restore_result, drain_result, cleanup_result) {
            (Ok(()), Ok(()), Ok(_), Ok(())) => Ok(session_id),
            (Err(error), Ok(()), _, _) => Err(error),
            (Ok(()), Err(error), _, _) => Err(error),
            (Ok(()), Ok(()), Err(error), _) => Err(error),
            (Ok(()), Ok(()), Ok(_), Err(error)) => Err(error),
            (Err(error), Err(restore_error), _, _) => {
                Err(error.context(format!("terminal restore also failed: {restore_error}")))
            }
        }
    }

    fn run_suspended_attached_session(&mut self, session_id: CodexSessionId) -> Result<()> {
        let pid = self
            .sessions
            .get(&session_id)
            .and_then(|session| session.state.pid())
            .or_else(|| self.hosts.get(&session_id).map(|host| host.pid))
            .ok_or_else(|| anyhow!("missing live session {}", session_id))?;
        if !self
            .sessions
            .get(&session_id)
            .is_some_and(|session| matches!(session.state, CodexSessionState::Attached { .. }))
        {
            self.session_mut(session_id)?
                .mark_attached(pid)
                .map_err(|error| anyhow!(error))?;
        }

        let relay_mode = enter_pass_through_mode()?;
        let relay_result = self.run_attached_relay(session_id);
        drop(relay_mode);

        if let Err(error) = relay_result {
            self.mark_session_failed(session_id, error.to_string());
            self.terminate_host(session_id);
            return Err(error);
        }

        Ok(())
    }

    fn insert_session(&mut self, descriptor: CodexResumeDescriptor) -> CodexSessionId {
        let session_id = CodexSessionId(self.next_session_id);
        self.next_session_id += 1;
        let session = CodexSession::new(session_id, descriptor, self.ring_capacity);
        self.sessions.insert(session_id, session);
        session_id
    }

    fn spawn_pty_host(&mut self, session_id: CodexSessionId) -> Result<()> {
        let descriptor = self
            .sessions
            .get(&session_id)
            .map(|session| session.descriptor.clone())
            .ok_or_else(|| anyhow!("unknown Codex session {}", session_id))?;
        let pty_system = native_pty_system();
        let pty_size = current_pty_size();
        let pair = pty_system.openpty(pty_size).with_context(|| {
            format!("failed to allocate PTY for thread {}", descriptor.thread_id)
        })?;
        let writer = pair.master.take_writer().with_context(|| {
            format!(
                "failed to open PTY writer for thread {}",
                descriptor.thread_id
            )
        })?;
        let reader = pair.master.try_clone_reader().with_context(|| {
            format!(
                "failed to clone PTY reader for thread {}",
                descriptor.thread_id
            )
        })?;
        let command = descriptor.pty_command_builder();

        info!(
            session_id = %session_id,
            thread_id = %descriptor.thread_id,
            command = %descriptor.display_command,
            rows = pty_size.rows,
            cols = pty_size.cols,
            "launching Codex resume in PTY-backed mode"
        );

        let child = pair.slave.spawn_command(command).with_context(|| {
            format!(
                "failed to spawn `{}` for thread {}",
                descriptor.display_command, descriptor.thread_id
            )
        })?;
        let pid = child.process_id().unwrap_or_default();
        let killer = child.clone_killer();
        self.session_mut(session_id)?
            .mark_attached(pid)
            .map_err(|error| anyhow!(error))?;
        self.hosts.insert(
            session_id,
            CodexSessionHost {
                pid,
                master: pair.master,
                writer: Arc::new(Mutex::new(writer)),
                killer,
            },
        );
        spawn_output_drain_thread(session_id, reader, self.event_tx.clone())?;
        spawn_wait_thread(session_id, child, self.event_tx.clone())?;
        Ok(())
    }

    fn run_attached_relay(&mut self, session_id: CodexSessionId) -> Result<()> {
        let writer = Arc::clone(
            &self
                .hosts
                .get(&session_id)
                .ok_or_else(|| anyhow!("missing PTY host for session {}", session_id))?
                .writer,
        );
        let (stop, input_thread, control_rx) = spawn_input_relay_thread(writer)?;
        let mut stdout = io::stdout().lock();
        let mut last_size = current_pty_size();

        loop {
            self.drain_background_events_for_attached(session_id, &mut stdout)?;
            if matches!(
                control_rx.try_recv(),
                Ok(InputRelayControl::DetachRequested)
            ) {
                self.session_mut(session_id)?
                    .mark_detached()
                    .map_err(|error| anyhow!(error))?;
                break;
            }
            if !self
                .sessions
                .get(&session_id)
                .is_some_and(|session| matches!(session.state, CodexSessionState::Attached { .. }))
            {
                break;
            }

            self.propagate_resize_if_needed(session_id, &mut last_size)?;
            self.wait_for_background_event(RELAY_POLL_INTERVAL, Some((session_id, &mut stdout)))?;
        }

        stop.store(true, Ordering::Relaxed);
        stdout.flush().context("failed to flush final PTY output")?;
        join_relay_thread(input_thread, "stdin relay")?;
        Ok(())
    }

    fn drain_background_events_for_attached(
        &mut self,
        attached_session_id: CodexSessionId,
        stdout: &mut dyn Write,
    ) -> Result<bool> {
        let mut changed = false;
        while let Ok(event) = self.event_rx.try_recv() {
            changed |= self.handle_background_event(event, Some((attached_session_id, stdout)))?;
        }
        Ok(changed)
    }

    fn wait_for_background_event(
        &mut self,
        timeout: Duration,
        attached: Option<(CodexSessionId, &mut dyn Write)>,
    ) -> Result<bool> {
        match self.event_rx.recv_timeout(timeout) {
            Ok(event) => self.handle_background_event(event, attached),
            Err(mpsc::RecvTimeoutError::Timeout) => Ok(false),
            Err(mpsc::RecvTimeoutError::Disconnected) => Ok(false),
        }
    }

    fn handle_background_event(
        &mut self,
        event: BackgroundEvent,
        attached: Option<(CodexSessionId, &mut dyn Write)>,
    ) -> Result<bool> {
        match event {
            BackgroundEvent::Output { session_id, bytes } => {
                let Some(session) = self.sessions.get_mut(&session_id) else {
                    return Ok(false);
                };
                session.record_pty_output(&bytes);
                if let Some((attached_session_id, stdout)) = attached
                    && attached_session_id == session_id
                    && matches!(session.state, CodexSessionState::Attached { .. })
                {
                    stdout
                        .write_all(&bytes)
                        .context("failed to write PTY output to stdout")?;
                    stdout.flush().context("failed to flush PTY output")?;
                }
                Ok(true)
            }
            BackgroundEvent::Exited { session_id, result } => {
                if let Some(session) = self.sessions.get_mut(&session_id) {
                    let _ = session.mark_exited(result);
                }
                self.hosts.remove(&session_id);
                Ok(true)
            }
            BackgroundEvent::Failed { session_id, error } => {
                self.mark_session_failed(session_id, error);
                self.terminate_host(session_id);
                self.hosts.remove(&session_id);
                Ok(true)
            }
        }
    }

    fn propagate_resize_if_needed(
        &self,
        session_id: CodexSessionId,
        last_size: &mut PtySize,
    ) -> Result<()> {
        let size = current_pty_size();
        if size == *last_size {
            return Ok(());
        }
        self.hosts
            .get(&session_id)
            .ok_or_else(|| anyhow!("missing PTY host for session {}", session_id))?
            .master
            .resize(size)
            .with_context(|| format!("failed to resize PTY for session {}", session_id))?;
        *last_size = size;
        Ok(())
    }

    fn live_session_id_for_thread(&self, thread_id: &str) -> Option<CodexSessionId> {
        self.sessions
            .iter()
            .rev()
            .find(|(_, session)| session.thread_id == thread_id && session.state.is_live())
            .map(|(session_id, _)| *session_id)
    }

    fn cleanup_terminal_sessions(&mut self) -> Result<()> {
        self.hosts.retain(|session_id, _| {
            self.sessions
                .get(session_id)
                .is_some_and(|session| session.state.is_live())
        });
        Ok(())
    }

    fn terminate_host(&mut self, session_id: CodexSessionId) {
        if let Some(host) = self.hosts.get_mut(&session_id) {
            let _ = host.killer.kill();
        }
    }

    fn mark_session_failed(&mut self, session_id: CodexSessionId, error: String) {
        if let Ok(session) = self.session_mut(session_id) {
            let _ = session.mark_failed(error);
        }
    }

    fn session_mut(&mut self, session_id: CodexSessionId) -> Result<&mut CodexSession> {
        self.sessions
            .get_mut(&session_id)
            .ok_or_else(|| anyhow!("unknown Codex session {}", session_id))
    }
}

impl Default for CodexSessionManager {
    fn default() -> Self {
        Self::new(DEFAULT_PTY_RING_BUFFER_CAPACITY)
    }
}

fn selected_thread_summary<'state>(
    state: &'state AppState,
    thread_id: &str,
) -> Option<&'state orcas_core::ipc::ThreadSummary> {
    state
        .thread_details
        .get(thread_id)
        .map(|thread| &thread.summary)
        .or_else(|| state.threads.iter().find(|thread| thread.id == thread_id))
}

fn render_display_command(
    program: &PathBuf,
    args: &[String],
    env_overrides: &BTreeMap<String, String>,
) -> String {
    let env_prefix = env_overrides
        .iter()
        .map(|(key, value)| format!("{key}={}", shell_escape(value)))
        .collect::<Vec<_>>();
    let mut parts = Vec::with_capacity(1 + args.len() + env_prefix.len());
    parts.extend(env_prefix);
    parts.push(shell_escape(&program.display().to_string()));
    parts.extend(args.iter().map(|value| shell_escape(value)));
    parts.join(" ")
}

fn shell_escape(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }
    if value.bytes().all(|byte| {
        byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'/' | b'.' | b':')
    }) {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn current_pty_size() -> PtySize {
    let (cols, rows) = terminal::size().unwrap_or((80, 24));
    PtySize {
        rows: rows.max(1),
        cols: cols.max(1),
        pixel_width: 0,
        pixel_height: 0,
    }
}

fn spawn_output_drain_thread(
    session_id: CodexSessionId,
    mut reader: Box<dyn Read + Send>,
    event_tx: mpsc::Sender<BackgroundEvent>,
) -> Result<()> {
    thread::Builder::new()
        .name(format!("orcas-codex-drain-{}", session_id.as_u64()))
        .spawn(move || {
            let mut buffer = [0_u8; 8192];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(read) => {
                        if event_tx
                            .send(BackgroundEvent::Output {
                                session_id,
                                bytes: buffer[..read].to_vec(),
                            })
                            .is_err()
                        {
                            break;
                        }
                    }
                    Err(error) if is_normal_pty_eof(&error) => break,
                    Err(error) => {
                        let _ = event_tx.send(BackgroundEvent::Failed {
                            session_id,
                            error: format!("failed to read PTY output: {error}"),
                        });
                        break;
                    }
                }
            }
        })
        .context("failed to spawn PTY output drain thread")?;
    Ok(())
}

fn spawn_wait_thread(
    session_id: CodexSessionId,
    mut child: Box<dyn Child + Send + Sync>,
    event_tx: mpsc::Sender<BackgroundEvent>,
) -> Result<()> {
    thread::Builder::new()
        .name(format!("orcas-codex-wait-{}", session_id.as_u64()))
        .spawn(move || match child.wait() {
            Ok(status) => {
                let _ = event_tx.send(BackgroundEvent::Exited {
                    session_id,
                    result: CodexExit::from(status),
                });
            }
            Err(error) => {
                let _ = event_tx.send(BackgroundEvent::Failed {
                    session_id,
                    error: format!("failed to wait for Codex session: {error}"),
                });
            }
        })
        .context("failed to spawn Codex wait thread")?;
    Ok(())
}

fn spawn_input_relay_thread(
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
) -> Result<(
    Arc<AtomicBool>,
    thread::JoinHandle<Result<()>>,
    mpsc::Receiver<InputRelayControl>,
)> {
    let stop = Arc::new(AtomicBool::new(false));
    let (control_tx, control_rx) = mpsc::channel();
    let stop_for_thread = Arc::clone(&stop);
    let handle = thread::Builder::new()
        .name("orcas-codex-stdin".to_string())
        .spawn(move || {
            #[cfg(unix)]
            let _stdin_flags = NonblockingStdinGuard::new()?;

            let stdin = io::stdin();
            let mut buffer = [0_u8; 1024];
            let mut pending_detach_prefix: Option<Instant> = None;

            loop {
                if stop_for_thread.load(Ordering::Relaxed) {
                    break;
                }

                if pending_detach_prefix
                    .is_some_and(|started_at| started_at.elapsed() >= DETACH_SEQUENCE_TIMEOUT)
                {
                    forward_input_bytes(&writer, &[DETACH_PREFIX])?;
                    pending_detach_prefix = None;
                }

                match read_stdin_chunk(&stdin, &mut buffer) {
                    Ok(0) => break,
                    Ok(read) => {
                        for byte in &buffer[..read] {
                            if pending_detach_prefix.is_some() {
                                if *byte == DETACH_SUFFIX
                                    || *byte == DETACH_SUFFIX.to_ascii_uppercase()
                                {
                                    let _ = control_tx.send(InputRelayControl::DetachRequested);
                                    pending_detach_prefix = None;
                                    continue;
                                }

                                forward_input_bytes(&writer, &[DETACH_PREFIX])?;
                                pending_detach_prefix = None;
                            }

                            if *byte == DETACH_PREFIX {
                                pending_detach_prefix = Some(Instant::now());
                            } else {
                                forward_input_bytes(&writer, &[*byte])?;
                            }
                        }
                    }
                    Err(error) if is_would_block(&error) => {
                        thread::sleep(INPUT_IDLE_SLEEP);
                    }
                    Err(error) => return Err(error).context("failed to read stdin for PTY relay"),
                }
            }

            if pending_detach_prefix.is_some() && !stop_for_thread.load(Ordering::Relaxed) {
                forward_input_bytes(&writer, &[DETACH_PREFIX])?;
            }

            Ok(())
        })
        .context("failed to spawn stdin relay thread")?;
    Ok((stop, handle, control_rx))
}

fn forward_input_bytes(writer: &Arc<Mutex<Box<dyn Write + Send>>>, bytes: &[u8]) -> Result<()> {
    let mut writer = writer
        .lock()
        .map_err(|_| anyhow!("PTY writer lock was poisoned"))?;
    writer
        .write_all(bytes)
        .context("failed to write stdin bytes into PTY")
}

fn join_relay_thread(handle: thread::JoinHandle<Result<()>>, label: &str) -> Result<()> {
    match handle.join() {
        Ok(result) => result.with_context(|| format!("{label} terminated with an error")),
        Err(_) => Err(anyhow!("{label} panicked")),
    }
}

#[cfg(unix)]
fn read_stdin_chunk(stdin: &io::Stdin, buffer: &mut [u8]) -> io::Result<usize> {
    use std::os::fd::AsFd;

    nix_read(stdin.as_fd(), buffer).map_err(io::Error::from)
}

#[cfg(not(unix))]
fn read_stdin_chunk(stdin: &io::Stdin, buffer: &mut [u8]) -> io::Result<usize> {
    let mut handle = stdin.lock();
    handle.read(buffer)
}

fn is_would_block(error: &io::Error) -> bool {
    error.kind() == io::ErrorKind::WouldBlock
        || matches!(error.raw_os_error(), Some(code) if code == libc_would_block())
}

fn is_normal_pty_eof(error: &io::Error) -> bool {
    #[cfg(unix)]
    {
        matches!(error.raw_os_error(), Some(code) if code == Errno::EIO as i32)
    }
    #[cfg(not(unix))]
    {
        let _ = error;
        false
    }
}

#[cfg(unix)]
fn libc_would_block() -> i32 {
    Errno::EAGAIN as i32
}

#[cfg(not(unix))]
fn libc_would_block() -> i32 {
    0
}

#[cfg(unix)]
struct NonblockingStdinGuard {
    original_flags: OFlag,
}

#[cfg(unix)]
impl NonblockingStdinGuard {
    fn new() -> Result<Self> {
        use std::os::fd::AsFd;

        let stdin = io::stdin();
        let fd = stdin.as_fd();
        let current_bits =
            fcntl(fd, FcntlArg::F_GETFL).context("failed to query stdin file status flags")?;
        let original_flags = OFlag::from_bits_truncate(current_bits);
        fcntl(fd, FcntlArg::F_SETFL(original_flags | OFlag::O_NONBLOCK))
            .context("failed to set stdin nonblocking mode")?;
        Ok(Self { original_flags })
    }
}

#[cfg(unix)]
impl Drop for NonblockingStdinGuard {
    fn drop(&mut self) {
        use std::os::fd::AsFd;

        let stdin = io::stdin();
        let fd = stdin.as_fd();
        let _ = fcntl(fd, FcntlArg::F_SETFL(self.original_flags));
    }
}

#[cfg(test)]
mod tests {
    use super::{
        CodexExit, CodexResumeDescriptor, CodexSession, CodexSessionId, CodexSessionManager,
        CodexSessionState, DEFAULT_PTY_RING_BUFFER_CAPACITY, MAX_SESSION_HISTORY_PER_THREAD,
    };
    use crate::app::AppState;
    use chrono::Utc;
    use orcas_core::ipc;
    use portable_pty::ExitStatus as PtyExitStatus;
    use std::path::PathBuf;

    #[test]
    fn session_transitions_cover_launch_attach_detach_reattach_exit() {
        let descriptor = sample_descriptor("thread-1");
        let mut session = CodexSession::new(CodexSessionId(1), descriptor, 16);
        assert_eq!(session.state, CodexSessionState::Launching);

        session.mark_attached(4242).expect("attach");
        assert_eq!(session.state, CodexSessionState::Attached { pid: 4242 });

        session.mark_detached().expect("detach");
        assert_eq!(session.state, CodexSessionState::Detached { pid: 4242 });

        session.mark_attached(4242).expect("reattach");
        assert_eq!(session.state, CodexSessionState::Attached { pid: 4242 });

        session
            .mark_exited(CodexExit {
                success: true,
                code: Some(0),
            })
            .expect("exit");
        assert_eq!(
            session.state,
            CodexSessionState::Exited {
                result: CodexExit {
                    success: true,
                    code: Some(0),
                },
            }
        );
    }

    #[test]
    fn detached_session_can_exit() {
        let descriptor = sample_descriptor("thread-1");
        let mut session = CodexSession::new(CodexSessionId(2), descriptor, 16);
        session.mark_attached(7).expect("attach");
        session.mark_detached().expect("detach");
        session
            .mark_exited(CodexExit {
                success: false,
                code: Some(1),
            })
            .expect("exit");
        assert!(matches!(session.state, CodexSessionState::Exited { .. }));
    }

    #[test]
    fn detached_session_can_fail() {
        let descriptor = sample_descriptor("thread-1");
        let mut session = CodexSession::new(CodexSessionId(3), descriptor, 16);
        session.mark_attached(7).expect("attach");
        session.mark_detached().expect("detach");
        session.mark_failed("boom").expect("failed");
        assert_eq!(
            session.state,
            CodexSessionState::Failed {
                error: "boom".to_string(),
            }
        );
    }

    #[test]
    fn recording_output_updates_bounded_buffer() {
        let descriptor = sample_descriptor("thread-1");
        let mut session = CodexSession::new(CodexSessionId(4), descriptor, 4);
        session.record_pty_output(b"abc");
        session.record_pty_output(b"def");
        assert_eq!(session.pty_output.snapshot(), b"cdef");
    }

    #[test]
    fn descriptor_uses_selected_thread_and_daemon_binary() {
        let mut state = AppState::default();
        state.selected_thread_id = Some("thread-1".to_string());
        state.daemon = Some(ipc::DaemonStatusResponse {
            socket_path: "/tmp/orcasd.sock".to_string(),
            metadata_path: "/tmp/orcasd.json".to_string(),
            codex_endpoint: "ws://127.0.0.1:4545".to_string(),
            codex_binary_path: "/usr/local/bin/codex".to_string(),
            upstream: orcas_core::ConnectionState {
                endpoint: "ws://127.0.0.1:4545".to_string(),
                status: "connected".to_string(),
                detail: None,
            },
            client_count: 1,
            known_threads: 1,
            runtime: ipc::DaemonRuntimeMetadata {
                pid: 42,
                started_at: Utc::now(),
                version: "0.1.0".to_string(),
                build_fingerprint: "fingerprint".to_string(),
                binary_path: "/usr/local/bin/orcasd".to_string(),
                socket_path: "/tmp/orcasd.sock".to_string(),
                metadata_path: "/tmp/orcasd.json".to_string(),
                git_commit: None,
            },
        });
        state.threads.push(ipc::ThreadSummary {
            id: "thread-1".to_string(),
            preview: "preview".to_string(),
            name: None,
            model_provider: "openai".to_string(),
            cwd: "/worktree".to_string(),
            status: "idle".to_string(),
            created_at: 0,
            updated_at: 0,
            scope: "orcas_managed".to_string(),
            archived: false,
            loaded_status: ipc::ThreadLoadedStatus::Idle,
            active_flags: Vec::new(),
            active_turn_id: None,
            last_seen_turn_id: None,
            recent_output: None,
            recent_event: None,
            turn_in_flight: false,
            monitor_state: ipc::ThreadMonitorState::Detached,
            last_sync_at: Utc::now(),
            source_kind: None,
            raw_summary: None,
        });

        let descriptor = CodexResumeDescriptor::for_selected_thread(&state).expect("descriptor");
        assert_eq!(descriptor.program, PathBuf::from("/usr/local/bin/codex"));
        assert_eq!(
            descriptor.args,
            vec!["resume", "--cd", "/worktree", "thread-1"]
        );
    }

    #[test]
    fn pty_exit_status_conversion_preserves_exit_code() {
        let exit = CodexExit::from(PtyExitStatus::with_exit_code(17));
        assert!(!exit.success);
        assert_eq!(exit.code, Some(17));
    }

    #[test]
    fn only_one_live_session_is_tracked_per_thread() {
        let mut manager = CodexSessionManager::default();
        let first_id = manager.insert_session(sample_descriptor("thread-1"));
        manager
            .sessions
            .get_mut(&first_id)
            .expect("session")
            .mark_attached(11)
            .expect("attach");

        let second_id = manager.insert_session(sample_descriptor("thread-1"));
        manager
            .sessions
            .get_mut(&second_id)
            .expect("session")
            .mark_exited(CodexExit {
                success: true,
                code: Some(0),
            })
            .expect("exit");

        assert_eq!(
            manager.live_session_id_for_thread("thread-1"),
            Some(first_id)
        );
    }

    #[test]
    fn thread_sessions_expose_recent_preview_and_bounded_history() {
        let mut manager = CodexSessionManager::default();
        for session_number in 0..(MAX_SESSION_HISTORY_PER_THREAD + 2) {
            let session_id = manager.insert_session(sample_descriptor("thread-1"));
            let session = manager.sessions.get_mut(&session_id).expect("session");
            session
                .mark_attached(100 + session_number as u32)
                .expect("attach");
            session.record_pty_output(format!("line-{session_number}\n").as_bytes());
            if session_number % 2 == 0 {
                session.mark_detached().expect("detach");
            } else {
                session
                    .mark_exited(CodexExit {
                        success: true,
                        code: Some(0),
                    })
                    .expect("exit");
            }
        }

        let history = manager.thread_sessions();
        let thread_sessions = history.get("thread-1").expect("thread history");
        assert_eq!(
            thread_sessions.sessions.len(),
            MAX_SESSION_HISTORY_PER_THREAD
        );
        assert_eq!(thread_sessions.sessions[0].output_preview.lines.len(), 1);
        assert!(thread_sessions.sessions[0].output_preview.lines[0].starts_with("line-"));
    }

    fn sample_descriptor(thread_id: &str) -> CodexResumeDescriptor {
        CodexResumeDescriptor {
            thread_id: thread_id.to_string(),
            program: PathBuf::from("/usr/local/bin/codex"),
            args: vec!["resume".to_string(), thread_id.to_string()],
            cwd: Some(PathBuf::from("/worktree")),
            env_overrides: Default::default(),
            display_command: format!("codex resume {thread_id}"),
        }
    }

    #[test]
    fn default_ring_capacity_is_non_zero() {
        assert!(DEFAULT_PTY_RING_BUFFER_CAPACITY > 0);
    }
}
