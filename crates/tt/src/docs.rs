use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use clap::{Command, CommandFactory, ColorChoice};

use crate::Cli;

pub fn export_cli_markdown(out: impl AsRef<Path>) -> Result<()> {
    let out = out.as_ref();
    let markdown = render_cli_markdown();
    if let Some(parent) = out.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    fs::write(out, markdown).with_context(|| format!("write {}", out.display()))?;
    Ok(())
}

pub fn render_cli_markdown() -> String {
    let mut out = String::new();
    let command = Cli::command().color(ColorChoice::Never);

    writeln!(out, "# TT CLI Reference").expect("write markdown");
    writeln!(out).expect("write markdown");
    writeln!(
        out,
        "Generated from the live `tt` Clap tree. Regenerate with `tt docs export-cli --out docs/CLI_REFERENCE.md`."
    )
    .expect("write markdown");
    writeln!(out).expect("write markdown");

    render_command_tree(&command, 2, &[], &mut out);
    out
}

fn render_command_tree(command: &Command, level: usize, prefix: &[String], out: &mut String) {
    let heading = "#".repeat(level);
    let command_path = command_path(command, prefix);

    writeln!(out, "{heading} `{command_path}`").expect("write markdown");
    writeln!(out).expect("write markdown");
    writeln!(out, "```text").expect("write markdown");
    let mut rendered_command = command.clone();
    let mut rendered = rendered_command.render_long_help().to_string();
    if !rendered.ends_with('\n') {
        rendered.push('\n');
    }
    out.push_str(&rendered);
    writeln!(out, "```").expect("write markdown");
    writeln!(out).expect("write markdown");

    for subcommand in command.get_subcommands() {
        if subcommand.is_hide_set() {
            continue;
        }
        if subcommand.get_name() == "TT" {
            continue;
        }
        let mut next_prefix = prefix.to_vec();
        next_prefix.push(command.get_name().to_string());
        render_command_tree(subcommand, level + 1, &next_prefix, out);
    }
}

fn command_path(command: &Command, prefix: &[String]) -> String {
    let mut parts = Vec::new();
    parts.extend(prefix.iter().cloned());
    collect_command_path(command, &mut parts);
    parts.join(" ")
}

fn collect_command_path(command: &Command, parts: &mut Vec<String>) {
    parts.push(command.get_name().to_string());
}

#[cfg(test)]
mod tests {
    use super::render_cli_markdown;

    #[test]
    fn renders_root_and_lane_sections() {
        let markdown = render_cli_markdown();
        assert!(markdown.contains("# TT CLI Reference"));
        assert!(markdown.contains("`tt`"));
        assert!(markdown.contains("`tt lane`"));
        assert!(markdown.contains("`tt docs`"));
        assert!(markdown.contains("`tt skill`"));
        assert!(markdown.contains("`tt todo`"));
        assert!(markdown.contains("`tt split`"));
        assert!(!markdown.contains("`tt roles`"));
        assert!(!markdown.contains("`tt supervisor`"));
    }
}
