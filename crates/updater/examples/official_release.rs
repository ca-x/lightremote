//! Opt-in live GitHub probe. Unit tests never contact the network.
use std::{env, fs, io, path::Path};

use lightremote_updater::{InstalledUpdate, Product, SessionGate, Updater};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut install = false;
    let mut expected = None;
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--install" => install = true,
            "--expect" => {
                expected = Some(
                    args.next()
                        .ok_or("--expect requires an exact release tag")?,
                )
            }
            _ => {
                return Err(
                    format!("unknown argument {arg}; use [--install] [--expect v0.1.1]").into(),
                );
            }
        }
    }
    let updater = Updater::new(Product::HeadlessCli, "0.0.0")?;
    let release = updater
        .check()
        .await?
        .ok_or("no official stable upgrade is available")?;
    if expected.as_deref().is_some_and(|tag| tag != release.tag()) {
        return Err(format!("expected {expected:?}, GitHub latest is {}", release.tag()).into());
    }
    println!(
        "official_release={} asset={} page={}",
        release.tag(),
        release.asset_name(),
        release.release_url()
    );
    if !install {
        return Ok(());
    }

    // No caller-provided destination is accepted. Everything created here remains
    // below this fresh temporary directory, including session locks and backups.
    let directory = tempfile::Builder::new()
        .prefix("lightremote-updater-probe-")
        .tempdir()?;
    make_private(directory.path())?;
    let destination = directory.path().join(if cfg!(windows) {
        "lightremote.exe"
    } else {
        "lightremote"
    });
    const OLD: &[u8] = b"isolated old LightRemote dummy; never executed\n";
    fs::write(&destination, OLD)?;
    let gate = SessionGate::new(&directory.path().join("sessions.lock"))?;
    let result = updater.install(&release, &destination, &gate).await?;
    match result {
        InstalledUpdate::Installed {
            version,
            destination,
            backup,
        } => {
            if fs::read(&backup)? != OLD || fs::read(&destination)? == OLD {
                return Err("isolated installation/backup verification failed".into());
            }
            println!(
                "verified_download=GitHub_digest+SHA256SUMS installed={version} destination={} backup={}",
                destination.display(),
                backup.display()
            );
        }
        InstalledUpdate::Pending(pending) => {
            if fs::read(&destination)? != OLD || fs::read(&pending.backup)? != OLD {
                return Err("pending transaction changed old destination".into());
            }
            println!(
                "verified_download=GitHub_digest+SHA256SUMS pending={} stage={} script={}",
                pending.version,
                pending.staged.display(),
                pending.script.display()
            );
            println!("Windows helper is prepared for inspection; this probe does not launch it");
        }
        _ => return Err("unrecognized install state".into()),
    }
    println!("retained_isolated_directory={}", directory.keep().display());
    Ok(())
}

fn make_private(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}
