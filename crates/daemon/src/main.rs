//! Independent headless service entry point; host lifecycle lives in `lightremote`.
mod config;
mod security;
#[cfg(any(windows, test))]
mod shutdown;
#[cfg(windows)]
mod windows;

use anyhow::{Context, Result};
use clap::Parser;
use std::{path::PathBuf, process::ExitCode};

#[derive(Parser)]
#[command(
    version,
    about = "LightRemote unattended host service (headless by default)"
)]
struct Args {
    /// Absolute path to a private, strictly validated JSON configuration.
    #[arg(long, value_name = "PATH")]
    config: PathBuf,
    /// Validate configuration and filesystem trust without network or service changes.
    #[arg(long)]
    check_config: bool,
    /// Run under Windows Service Control Manager (does not install a service).
    #[arg(long)]
    service: bool,
}

fn main() -> ExitCode {
    let result = run(Args::parse());
    let code = config::exit_code(&result);
    if let Err(error) = result {
        if code == config::RESTART_EXIT_CODE {
            eprintln!("Update installed; service supervisor must restart the daemon.");
        } else {
            // Display only the context, never an error chain from the network.
            eprintln!("{error}");
        }
    }
    ExitCode::from(code)
}

fn run(args: Args) -> Result<()> {
    #[cfg(not(windows))]
    anyhow::ensure!(
        !args.service,
        "--service requires Windows SCM; use console mode with systemd or launchd"
    );
    if args.check_config {
        config::load(&args.config, args.service)?;
        println!("Configuration is valid.");
        return Ok(());
    }
    #[cfg(windows)]
    if args.service {
        return windows::dispatch(args.config);
    }
    let options = config::load(&args.config, false)?.prepare()?;
    runtime()?
        .block_on(lightremote::host::run(options))
        .context("Host engine stopped with an error; check local policy and relay availability")
}

fn runtime() -> Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("cannot start daemon runtime")
}
