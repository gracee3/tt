use std::collections::VecDeque;

const DEFAULT_PREVIEW_SOURCE_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CodexOutputPreview {
    pub lines: Vec<String>,
    pub truncated: bool,
    pub control_sequences_removed: bool,
}

impl CodexOutputPreview {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }
}

#[must_use]
pub fn render_preview_from_pty_bytes(
    bytes: &[u8],
    max_lines: usize,
    max_line_width: usize,
) -> CodexOutputPreview {
    if bytes.is_empty() || max_lines == 0 || max_line_width == 0 {
        return CodexOutputPreview::default();
    }

    let mut truncated = false;
    let source = if bytes.len() > DEFAULT_PREVIEW_SOURCE_BYTES {
        truncated = true;
        &bytes[bytes.len() - DEFAULT_PREVIEW_SOURCE_BYTES..]
    } else {
        bytes
    };

    let (sanitized, control_sequences_removed) = strip_terminal_bytes(source);
    let text = String::from_utf8_lossy(&sanitized);
    let mut lines = VecDeque::with_capacity(max_lines);

    for raw_line in text.lines() {
        let compact = raw_line.split_whitespace().collect::<Vec<_>>().join(" ");
        let compact = compact.trim();
        if compact.is_empty() {
            continue;
        }

        let rendered = trim_to_recent_width(compact, max_line_width, &mut truncated);
        if lines.len() == max_lines {
            lines.pop_front();
            truncated = true;
        }
        lines.push_back(rendered);
    }

    CodexOutputPreview {
        lines: lines.into_iter().collect(),
        truncated,
        control_sequences_removed,
    }
}

fn strip_terminal_bytes(bytes: &[u8]) -> (Vec<u8>, bool) {
    let mut output = Vec::with_capacity(bytes.len());
    let mut control_sequences_removed = false;
    let mut state = EscapeState::Text;

    for &byte in bytes {
        match state {
            EscapeState::Text => match byte {
                0x1b => {
                    state = EscapeState::Esc;
                    control_sequences_removed = true;
                }
                b'\n' => output.push(b'\n'),
                b'\r' => {
                    if !output.ends_with(b"\n") {
                        output.push(b'\n');
                    }
                }
                b'\t' => output.push(b' '),
                0x20..=0x7e | 0x80..=0xff => output.push(byte),
                _ => {
                    control_sequences_removed = true;
                }
            },
            EscapeState::Esc => match byte {
                b'[' => {
                    state = EscapeState::Csi;
                }
                b']' => {
                    state = EscapeState::Osc;
                }
                _ => {
                    state = EscapeState::Text;
                }
            },
            EscapeState::Csi => {
                if (0x40..=0x7e).contains(&byte) {
                    state = EscapeState::Text;
                }
            }
            EscapeState::Osc => match byte {
                0x07 => state = EscapeState::Text,
                0x1b => state = EscapeState::OscEsc,
                _ => {}
            },
            EscapeState::OscEsc => {
                state = if byte == b'\\' {
                    EscapeState::Text
                } else {
                    EscapeState::Osc
                };
            }
        }
    }

    (output, control_sequences_removed)
}

fn trim_to_recent_width(line: &str, width: usize, truncated: &mut bool) -> String {
    let char_count = line.chars().count();
    if char_count <= width {
        return line.to_string();
    }

    *truncated = true;
    if width <= 3 {
        return line
            .chars()
            .rev()
            .take(width)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
    }

    let tail_width = width - 3;
    let tail = line
        .chars()
        .rev()
        .take(tail_width)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<String>();
    format!("...{tail}")
}

#[derive(Clone, Copy)]
enum EscapeState {
    Text,
    Esc,
    Csi,
    Osc,
    OscEsc,
}

#[cfg(test)]
mod tests {
    use super::{CodexOutputPreview, render_preview_from_pty_bytes};

    #[test]
    fn preview_keeps_only_recent_lines() {
        let preview =
            render_preview_from_pty_bytes(b"line-1\nline-2\nline-3\nline-4\nline-5\n", 3, 40);
        assert_eq!(
            preview,
            CodexOutputPreview {
                lines: vec![
                    "line-3".to_string(),
                    "line-4".to_string(),
                    "line-5".to_string()
                ],
                truncated: true,
                control_sequences_removed: false,
            }
        );
    }

    #[test]
    fn preview_strips_common_ansi_sequences() {
        let preview = render_preview_from_pty_bytes(
            b"\x1b[2J\x1b[Hprompt\r\n\x1b[31merror\x1b[0m\r\n",
            3,
            40,
        );
        assert_eq!(
            preview.lines,
            vec!["prompt".to_string(), "error".to_string()]
        );
        assert!(preview.control_sequences_removed);
    }

    #[test]
    fn preview_bounds_line_width_to_recent_text() {
        let preview = render_preview_from_pty_bytes(b"prefix-prefix-prefix-suffix\n", 2, 12);
        assert_eq!(preview.lines, vec!["...ix-suffix".to_string()]);
        assert!(preview.truncated);
    }

    #[test]
    fn preview_ignores_blank_and_control_only_lines() {
        let preview = render_preview_from_pty_bytes(b"\x00\x01\r\n \r\nhello\tworld\r\n", 3, 40);
        assert_eq!(preview.lines, vec!["hello world".to_string()]);
        assert!(preview.control_sequences_removed);
    }
}
