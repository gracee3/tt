//! Codex CLI session infrastructure for the TUI.
//!
//! The TUI keeps two separate sources of information:
//! - Orcas daemon/app-server notifications remain authoritative for thread and turn lifecycle.
//! - Local Codex CLI session state only tracks process ownership and recent PTY bytes.
//!
//! The current slices implement PTY-backed attach, detach/reattach, and a bounded recent-output
//! preview derived from the same PTY bytes. That preview is intentionally best-effort only. It is
//! useful operator context, not an attempt to reconstruct ratatui screen state or infer workflow
//! lifecycle from terminal output.

pub mod preview;
pub mod ring_buffer;
pub mod session;
pub mod terminal;

pub use preview::{CodexOutputPreview, render_preview_from_pty_bytes};
pub use ring_buffer::PtyRingBuffer;
pub use session::{
    CodexResumeDescriptor, CodexResumeDescriptorError, CodexSession, CodexSessionId,
    CodexSessionManager, CodexSessionState, CodexThreadSessionSummary, CodexThreadSessions,
    DEFAULT_PTY_RING_BUFFER_CAPACITY,
};
pub use terminal::{OrcasTerminal, SuspendedOrcasTerminal, suspend_terminal};
