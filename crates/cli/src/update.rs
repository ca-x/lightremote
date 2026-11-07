//! Explicit GitHub update commands and idle resident-host checks.
use anyhow::Result;
use lightremote_updater::{InstalledUpdate, Product, SessionGate, Updater};
use std::{fmt, path::Path};

#[derive(Clone, Copy, Debug)]
pub enum Mode {
    Check,
    Install,
}

#[derive(Debug)]
pub struct RestartRequired;
impl fmt::Display for RestartRequired {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("verified update prepared; restart required (exit 75)")
    }
}
impl std::error::Error for RestartRequired {}

fn updater() -> Result<Updater> {
    let product = if cfg!(feature = "desktop") {
        Product::DesktopCli
    } else {
        Product::HeadlessCli
    };
    Ok(Updater::new(product, env!("CARGO_PKG_VERSION"))?)
}

pub async fn run(mode: Mode) -> Result<()> {
    let updater = updater()?;
    let Some(release) = updater.check().await? else {
        println!("No newer official GitHub stable release for this product and architecture.");
        return Ok(());
    };
    println!(
        "GitHub stable {}: {}",
        release.version(),
        release.asset_name()
    );
    println!("{}", release.release_url());
    if matches!(mode, Mode::Install) {
        let executable = std::env::current_exe()?;
        let gate = SessionGate::for_executable(&executable)?;
        finish(updater.install(&release, &executable, &gate).await?)?;
        return Err(RestartRequired.into());
    }
    Ok(())
}

/// Returns true only after a verified replacement or an explicitly launched
/// Windows handoff. A running process must then leave, never resume hosting.
pub(crate) async fn install_if_available(
    executable: &Path,
    gate: &SessionGate,
    product: Product,
) -> Result<bool> {
    let updater = Updater::new(product, env!("CARGO_PKG_VERSION"))?;
    let Some(release) = updater.check().await? else {
        return Ok(false);
    };
    finish(updater.install(&release, executable, gate).await?)?;
    Ok(true)
}

fn finish(update: InstalledUpdate) -> Result<()> {
    match update {
        InstalledUpdate::Installed { version, .. } => eprintln!(
            "Verified GitHub release {version} installed; the running process must restart. The previous executable is retained as a backup."
        ),
        InstalledUpdate::Pending(pending) => {
            pending.launch()?;
            eprintln!(
                "Verified GitHub update staged; the Windows helper will replace the executable after this process exits. Restart after replacement completes."
            );
        }
        _ => anyhow::bail!("unsupported update completion state"),
    }
    Ok(())
}
