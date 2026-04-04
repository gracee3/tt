#![allow(unused_crate_dependencies)]

use std::env;
use std::path::PathBuf;
use std::process::ExitCode;

use orcas_codex::CodexContractSnapshot;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args_os().skip(1);
    let mut root = env::var_os("CODEX_REPO_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/home/emmy/openai/codex/codex-rs"));
    let mut out = None;
    let mut full = false;

    while let Some(arg) = args.next() {
        match arg.to_string_lossy().as_ref() {
            "--root" => {
                root = args
                    .next()
                    .map(PathBuf::from)
                    .ok_or("--root requires a value")?;
            }
            "--out" => {
                out = Some(
                    args.next()
                        .map(PathBuf::from)
                        .ok_or("--out requires a value")?,
                );
            }
            "--full" => {
                full = true;
            }
            "--help" | "-h" => {
                println!("Usage: codex-contract-sync [--root PATH] [--out FILE] [--full]");
                return Ok(());
            }
            other => {
                return Err(format!("unrecognized argument: {other}").into());
            }
        }
    }

    let snapshot = CodexContractSnapshot::load_from_codex_repo(&root)?;
    if let Some(out) = out {
        if full {
            snapshot.write_pretty_json(out)?;
        } else {
            snapshot.inventory().write_pretty_json(out)?;
        }
    } else {
        if full {
            println!("{}", snapshot.to_pretty_json()?);
        } else {
            println!("{}", snapshot.inventory().to_pretty_json()?);
        }
    }
    Ok(())
}
