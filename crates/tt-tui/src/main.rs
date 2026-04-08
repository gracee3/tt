#![allow(unused_crate_dependencies)]

use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;

#[derive(Debug, Parser)]
struct Cli {
    /// Run the interactive dashboard shell instead of printing a single snapshot.
    #[arg(long)]
    interactive: bool,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let cwd = std::env::current_dir()?;
    if cli.interactive {
        tt_tui::run_interactive(PathBuf::from(cwd))?;
    } else {
        let snapshot = tt_tui::load_snapshot(PathBuf::from(cwd))?;
        println!("{}", tt_tui::render_dashboard(&snapshot));
    }
    Ok(())
}
