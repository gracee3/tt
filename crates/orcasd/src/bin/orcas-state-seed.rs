#![allow(unused_crate_dependencies)]

use std::fs;
use std::path::PathBuf;

use anyhow::Context;
use clap::Parser;
use orcas_core::StoredState;

#[derive(Debug, Parser)]
struct Args {
    /// Seed or mutated state.json input to normalize through StoredState.
    #[arg(long)]
    input: PathBuf,
    /// Destination file. Defaults to in-place rewrite of --input.
    #[arg(long)]
    output: Option<PathBuf>,
}

fn normalize_state_file(input: &PathBuf, output: &PathBuf) -> anyhow::Result<()> {
    let raw = fs::read_to_string(input).with_context(|| format!("read {}", input.display()))?;
    let state = StoredState::from_json_str(&raw)
        .with_context(|| format!("parse {}", input.display()))?;
    let mut encoded = state
        .to_pretty_json()
        .with_context(|| format!("serialize {}", input.display()))?;
    encoded.push('\n');
    fs::write(output, encoded).with_context(|| format!("write {}", output.display()))?;
    Ok(())
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let output = args.output.unwrap_or_else(|| args.input.clone());
    normalize_state_file(&args.input, &output)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::normalize_state_file;
    use orcas_core::StoredState;
    use uuid::Uuid;

    #[test]
    fn normalize_state_file_round_trips_seeded_fixture() {
        let dir = tempdir().expect("temp dir");
        let input = dir.path().join(format!("seed-{}.json", Uuid::new_v4()));
        let output = dir.path().join(format!("normalized-{}.json", Uuid::new_v4()));
        fs::write(
            &input,
            r#"{
  "registry": {
    "threads": {},
    "last_connected_endpoint": null
  },
  "collaboration": {
    "workstreams": {},
    "work_units": {}
  }
}
"#,
        )
        .expect("write input");

        normalize_state_file(&input, &output).expect("normalize fixture");

        let raw = fs::read_to_string(&output).expect("read output");
        assert!(raw.ends_with('\n'));
        let (state, needs_normalization) =
            StoredState::from_json_str_with_normalization(&raw).expect("parse output");
        assert!(!needs_normalization);
        assert!(state.thread_views.is_empty());
        assert!(state.turn_states.is_empty());
    }
}
