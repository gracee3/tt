//! Codex CLI session infrastructure for the TUI.
//!
//! The TUI keeps two separate sources of information:
//! - Orcas daemon/app-server notifications remain authoritative for thread and turn lifecycle.
//! - Local Codex CLI session state only tracks process ownership and recent PTY bytes.
//!
//! This first slice implements the launch descriptor, bounded PTY ring buffer, session state model,
//! and terminal suspend/restore scaffolding needed for suspend-and-pass-through mode. The current
//! attached launch path intentionally hands terminal ownership directly to `codex resume ...`
//! instead of attempting to proxy an interactive PTY inside ratatui.
//!
//! Future detach/reattach work can add a PTY-backed launcher that drains output into the same
//! [`ring_buffer::PtyRingBuffer`] and transitions sessions into [`session::CodexSessionState::Detached`]
//! without redesigning the TUI-facing model.

pub mod ring_buffer;
pub mod session;
pub mod terminal;

pub use ring_buffer::PtyRingBuffer;
pub use session::{
    CodexResumeDescriptor, CodexResumeDescriptorError, CodexSession, CodexSessionId,
    CodexSessionManager, CodexSessionState, DEFAULT_PTY_RING_BUFFER_CAPACITY,
};
pub use terminal::{OrcasTerminal, SuspendedOrcasTerminal, suspend_terminal};
