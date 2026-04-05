#![allow(warnings)]

use std::env;
use std::path::PathBuf;
use std::process::ExitCode;

use tt_runtime::{TTContractSnapshot, default_tt_repo_root};

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
    let mut root = env::var_os("RUNTIME_REPO_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(default_tt_repo_root);
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
                println!("Usage: tt-contract-sync [--root PATH] [--out FILE] [--full]");
                println!("If --root is omitted, TT auto-discovers the source checkout.");
                return Ok(());
            }
            other => {
                return Err(format!("unrecognized argument: {other}").into());
            }
        }
    }

    let snapshot = TTContractSnapshot::load_from_tt_repo(&root)?;
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
