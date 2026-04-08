#![allow(unused_crate_dependencies)]

use std::path::PathBuf;

use anyhow::Result;

fn main() -> Result<()> {
    let cwd = std::env::current_dir()?;
    let snapshot = tt_tui::load_snapshot(PathBuf::from(cwd))?;
    println!("{}", tt_tui::render_dashboard(&snapshot));
    Ok(())
}
