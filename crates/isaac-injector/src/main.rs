use std::{path::PathBuf, process::ExitCode};

use clap::Parser;
use tractor_beam_isaac_injector::{InjectorError, inject, inject_guarded, write_failure_report};

#[derive(Debug, Parser)]
#[command(version, about = "Inject Tractor Beam Native Hook into Isaac")]
struct Args {
    #[arg(long)]
    pid: u32,
    #[arg(long)]
    dll: PathBuf,
    #[arg(long, hide = true)]
    result_file: Option<PathBuf>,
    #[arg(long, hide = true)]
    guard_file: Option<PathBuf>,
}

fn main() -> ExitCode {
    let args = Args::parse();
    let result = args.guard_file.as_deref().map_or_else(
        || inject(args.pid, &args.dll),
        |guard| inject_guarded(args.pid, &args.dll, guard),
    );
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            if let Some(path) = &args.result_file {
                let _ = write_failure_report(path, &error);
            }
            eprintln!("{error}");
            if matches!(error, InjectorError::UnsupportedPlatform) {
                ExitCode::from(2)
            } else {
                ExitCode::from(1)
            }
        }
    }
}
