use std::{path::PathBuf, process::ExitCode};

use basement_isaac_injector::{InjectorError, inject};
use clap::Parser;

#[derive(Debug, Parser)]
#[command(version, about = "Inject Basement Bridge Native Hook into Isaac")]
struct Args {
    #[arg(long)]
    pid: u32,
    #[arg(long)]
    dll: PathBuf,
}

fn main() -> ExitCode {
    let args = Args::parse();
    match inject(args.pid, &args.dll) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            if matches!(error, InjectorError::UnsupportedPlatform) {
                ExitCode::from(2)
            } else {
                ExitCode::from(1)
            }
        }
    }
}
