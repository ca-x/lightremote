use std::{
    fs::{self, File},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

use semver::Version;
use sha2::{Digest, Sha256};
use tempfile::NamedTempFile;

use crate::{Error, Release, Result, SessionGate, archive, binary};

/// A completed replacement or an explicit Windows shutdown/deferred-install state.
#[derive(Debug)]
#[non_exhaustive]
pub enum InstalledUpdate {
    /// The destination was replaced. The current process must be explicitly restarted.
    Installed {
        /// Installed stable version; the running process still has its original version.
        version: String,
        /// Canonical executable path.
        destination: PathBuf,
        /// Durable copy of the executable from immediately before replacement.
        backup: PathBuf,
    },
    /// Verified files and a helper are ready; installation has not happened yet.
    Pending(PendingUpdate),
}

/// An auditable Windows transaction. No helper process is launched automatically.
///
/// After explicit restart consent, run `powershell.exe -NoProfile -File <script>`
/// using separate process arguments and exit the original application. The helper
/// waits for the original process, locks the admission gate, rechecks SHA-256, and
/// calls .NET File.Replace. It never elevates or automatically restarts the app.
#[derive(Debug)]
#[non_exhaustive]
pub struct PendingUpdate {
    /// Verified target stable version.
    pub version: String,
    /// Existing executable to replace after shutdown.
    pub destination: PathBuf,
    /// Verified sibling candidate retained for the helper.
    pub staged: PathBuf,
    /// Recoverable old executable, also used by File.Replace.
    pub backup: PathBuf,
    /// UTF-8 PowerShell script; execution policy remains controlled by the user.
    pub script: PathBuf,
    /// Helper readiness and final outcome, retained for restart/recovery inspection.
    pub status: PathBuf,
    script_digest: String,
}

impl PendingUpdate {
    /// Verifies and starts the local helper, then returns without stopping this process.
    ///
    /// The caller must explicitly enter its restart state and exit after success.
    /// Startup waits for helper readiness, not completion; retain status/script/backup.
    /// Windows execution policy remains effective; no policy bypass is requested.
    ///
    /// # Errors
    /// Rejects a changed helper or failure to create the PowerShell process.
    /// On other platforms this returns [`Error::UnsupportedTarget`].
    pub fn launch(&self) -> Result<()> {
        #[cfg(windows)]
        {
            use std::{
                os::windows::process::CommandExt,
                process::{Command, Stdio},
                time::{Duration, Instant},
            };
            let metadata = fs::symlink_metadata(&self.script)?;
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                return Err(Error::UnsafePath(self.script.clone()));
            }
            if file_digest(&mut File::open(&self.script)?)? != self.script_digest {
                return Err(Error::Digest);
            }
            let root = std::env::var_os("SystemRoot")
                .ok_or(Error::Metadata("Windows system directory is unavailable"))?;
            let powershell =
                PathBuf::from(root).join("System32/WindowsPowerShell/v1.0/powershell.exe");
            if !powershell.is_absolute() {
                return Err(Error::UnsafePath(powershell));
            }
            let mut child = Command::new(powershell)
                .arg("-NoProfile")
                .arg("-NonInteractive")
                .arg("-File")
                .arg(&self.script)
                .creation_flags(0x0800_0000) // CREATE_NO_WINDOW: survive parent console shutdown.
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()?;
            let ready = format!("ready:{}", child.id());
            let deadline = Instant::now() + Duration::from_secs(15);
            loop {
                if fs::read_to_string(&self.status).is_ok_and(|status| status == ready) {
                    return Ok(());
                }
                if let Some(status) = child.try_wait()? {
                    return Err(Error::HelperFailed(status.code()));
                }
                if Instant::now() >= deadline {
                    child.kill()?;
                    let _ = child.wait();
                    return Err(Error::HelperFailed(None));
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
        #[cfg(not(windows))]
        {
            // The digest is retained on every platform so Windows preparation is testable.
            let _ = &self.script_digest;
            Err(Error::UnsupportedTarget)
        }
    }
}

pub(crate) fn prepare_and_commit(
    release: &Release,
    download: NamedTempFile,
    destination: &Path,
    gate: &SessionGate,
) -> Result<InstalledUpdate> {
    prepare_with(release, download, destination, gate, atomic_replace)
}

fn prepare_with(
    release: &Release,
    mut download: NamedTempFile,
    destination: &Path,
    gate: &SessionGate,
    replace: impl FnOnce(&Path, &Path) -> io::Result<()>,
) -> Result<InstalledUpdate> {
    let destination = safe_destination(destination)?;
    let parent = destination
        .parent()
        .ok_or_else(|| Error::UnsafePath(destination.clone()))?;
    let mut staged = tempfile::Builder::new()
        .prefix(".lightremote-stage-")
        .tempfile_in(parent)?;
    archive::extract(release, download.as_file_mut(), staged.as_file_mut())?;
    binary::validate(staged.as_file_mut(), release.target)?;
    executable_permissions(staged.as_file())?;
    staged.as_file().sync_all()?;
    // No await or new session admission is possible after this acquisition.
    let _exclusive = gate.exclusive()?;
    safe_destination(&destination)?;
    let mut backup = tempfile::Builder::new()
        .prefix(".lightremote-backup-")
        .tempfile_in(parent)?;
    let mut old = File::open(&destination)?;
    io::copy(&mut old, backup.as_file_mut())?;
    fs::set_permissions(backup.path(), old.metadata()?.permissions())?;
    backup.as_file().sync_all()?;
    let old_digest = file_digest(backup.as_file_mut())?;
    let candidate_digest = file_digest(staged.as_file_mut())?;
    record_version(gate, &release.version, &old_digest, &candidate_digest)?;
    if file_digest(&mut File::open(&destination)?)? != old_digest {
        return Err(Error::UnsafePath(destination));
    }
    if release.target.windows() {
        defer(
            release,
            staged,
            backup,
            &destination,
            gate,
            old_digest,
            candidate_digest,
        )
    } else {
        // Keep a backup before commit; never rename the old executable out of place.
        let (_file, backup_path) = backup.keep().map_err(|error| Error::Io(error.error))?;
        if let Err(error) = replace(staged.path(), &destination) {
            // The old executable is still at destination. A preserved backup is useful
            // for diagnosis, and removing it must never mask the original error.
            return Err(Error::Io(error));
        }
        // The rename is the commit point. A subsequent directory sync failure must not
        // be reported as an uncommitted failure: the backup remains recoverable.
        let _ = File::open(parent).and_then(|directory| directory.sync_all());
        Ok(InstalledUpdate::Installed {
            version: release.version(),
            destination,
            backup: backup_path,
        })
    }
}

// Append before commit so a running old process cannot install a cached older
// release over an already updated destination. Failed preparations leave harmless
// records: only the digest of the actual destination establishes its version.
fn record_version(
    gate: &SessionGate,
    next: &Version,
    installed_digest: &str,
    candidate_digest: &str,
) -> Result<()> {
    const JOURNAL_LIMIT: u64 = 128 * 1024;
    let mut journal = gate.journal()?;
    let mut records = String::new();
    (&mut journal)
        .take(JOURNAL_LIMIT + 1)
        .read_to_string(&mut records)?;
    if records.len() as u64 > JOURNAL_LIMIT {
        return Err(Error::SizeLimit(JOURNAL_LIMIT));
    }
    for line in records.lines() {
        let (version, digest) = line
            .split_once(' ')
            .ok_or(Error::Metadata("invalid installation version journal"))?;
        let version = Version::parse(version)?;
        if !version.pre.is_empty()
            || digest.len() != 64
            || !digest.bytes().all(|c| c.is_ascii_hexdigit())
        {
            return Err(Error::Metadata("invalid installation version journal"));
        }
        if digest == installed_digest {
            crate::release::ensure_upgrade(&version, next)?;
        }
    }
    let record = format!("{next} {candidate_digest}\n");
    if (records.len() + record.len()) as u64 > JOURNAL_LIMIT {
        return Err(Error::SizeLimit(JOURNAL_LIMIT));
    }
    if !records.lines().any(|line| line == record.trim_end()) {
        journal.seek(SeekFrom::End(0))?;
        journal.write_all(record.as_bytes())?;
        journal.sync_all()?;
    }
    Ok(())
}

fn safe_destination(path: &Path) -> Result<PathBuf> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(Error::UnsafePath(path.to_owned()));
    }
    // Never copy a setuid/setgid executable and silently preserve its privileges.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o6000 != 0 {
            return Err(Error::UnsafePath(path.to_owned()));
        }
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let name = path
        .file_name()
        .ok_or_else(|| Error::UnsafePath(path.to_owned()))?;
    Ok(fs::canonicalize(parent)?.join(name))
}

fn executable_permissions(file: &File) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o755))?;
    }
    #[cfg(not(unix))]
    let _ = file;
    Ok(())
}

fn atomic_replace(source: &Path, destination: &Path) -> io::Result<()> {
    fs::rename(source, destination)
}

fn file_digest(file: &mut File) -> Result<String> {
    file.seek(SeekFrom::Start(0))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn ps_literal(path: &Path) -> Result<String> {
    let value = path
        .to_str()
        .ok_or_else(|| Error::UnsafePath(path.to_owned()))?;
    if value.chars().any(char::is_control) {
        return Err(Error::UnsafePath(path.to_owned()));
    }
    Ok(format!("'{}'", value.replace('\'', "''")))
}

fn defer(
    release: &Release,
    staged: NamedTempFile,
    backup: NamedTempFile,
    destination: &Path,
    gate: &SessionGate,
    old_hash: String,
    new_hash: String,
) -> Result<InstalledUpdate> {
    let parent = destination
        .parent()
        .ok_or_else(|| Error::UnsafePath(destination.to_owned()))?;
    let mut script = tempfile::Builder::new()
        .prefix(".lightremote-install-")
        .suffix(".ps1")
        .tempfile_in(parent)?;
    let mut status = tempfile::Builder::new()
        .prefix(".lightremote-install-")
        .suffix(".status")
        .tempfile_in(parent)?;
    status.write_all(b"pending")?;
    status.as_file().sync_all()?;
    let text = windows_script(
        staged.path(),
        destination,
        backup.path(),
        gate.path(),
        status.path(),
        &old_hash,
        &new_hash,
    )?;
    script.write_all(b"\xef\xbb\xbf")?; // Windows PowerShell 5.1 needs a BOM for Unicode paths.
    script.write_all(text.as_bytes())?;
    script.as_file().sync_all()?;
    let script_digest = file_digest(script.as_file_mut())?;
    // Convert all three only after every fallible content/path check succeeded.
    let (_stage_file, staged) = staged.keep().map_err(|error| Error::Io(error.error))?;
    let (_backup_file, backup) = backup.keep().map_err(|error| Error::Io(error.error))?;
    let (_script_file, script) = script.keep().map_err(|error| Error::Io(error.error))?;
    let (_status_file, status) = status.keep().map_err(|error| Error::Io(error.error))?;
    Ok(InstalledUpdate::Pending(PendingUpdate {
        version: release.version(),
        destination: destination.to_owned(),
        staged,
        backup,
        script,
        status,
        script_digest,
    }))
}

fn windows_script(
    staged: &Path,
    destination: &Path,
    backup: &Path,
    gate: &Path,
    status: &Path,
    old_hash: &str,
    new_hash: &str,
) -> Result<String> {
    let staged = ps_literal(staged)?;
    let destination = ps_literal(destination)?;
    let backup = ps_literal(backup)?;
    let gate = ps_literal(gate)?;
    let status = ps_literal(status)?;
    let owner = std::process::id();
    Ok(format!(
        r#"# LightRemote deferred update: TLS-distributed SHA-256 integrity, no publisher signature.
# Review this script, then launch with powershell.exe -NoProfile -File <script>.
# No elevation, execution-policy bypass, downloads, or automatic restart.
$ErrorActionPreference = 'Stop'
$stage = {staged}
$destination = {destination}
$backup = {backup}
$gate = {gate}
$status = {status}
$oldHash = '{old_hash}'
$newHash = '{new_hash}'
$ownerId = {owner}
function Assert-Regular([string]$path) {{
    $attributes = [IO.File]::GetAttributes($path)
    if (($attributes -band [IO.FileAttributes]::Directory) -or ($attributes -band [IO.FileAttributes]::ReparsePoint)) {{
        throw 'Unsafe update path'
    }}
}}
function Hash([string]$path) {{
    $stream = [IO.File]::OpenRead($path)
    $sha = [Security.Cryptography.SHA256]::Create()
    try {{ return [BitConverter]::ToString($sha.ComputeHash($stream)).Replace('-', '').ToLowerInvariant() }}
    finally {{ $sha.Dispose(); $stream.Dispose() }}
}}
try {{
# Signal that PowerShell accepted this verified helper under its execution policy.
Assert-Regular $status
[IO.File]::WriteAllText($status, ('ready:' + $PID))
# Bind a Process object and bound the wait. A reused PID only delays/fails closed.
$owner = Get-Process -Id $ownerId -ErrorAction SilentlyContinue
if ($null -ne $owner -and -not $owner.WaitForExit(180000)) {{ throw 'Original process has not exited' }}
Assert-Regular $gate
$lock = [IO.File]::Open($gate, [IO.FileMode]::Open, [IO.FileAccess]::ReadWrite, [IO.FileShare]::ReadWrite)
$locked = $false
$deadline = [DateTime]::UtcNow.AddSeconds(180)
try {{
    # fs2 locks the whole file. Locking byte zero conflicts with both session shared
    # locks and another helper's exclusive lock, even when the file is empty.
    while (-not $locked) {{
        try {{ $lock.Lock(0, 1); $locked = $true }}
        catch [IO.IOException] {{
            if ([DateTime]::UtcNow -ge $deadline) {{ throw 'Active sessions prevent update' }}
            Start-Sleep -Milliseconds 200
        }}
    }}
    Assert-Regular $stage
    Assert-Regular $destination
    Assert-Regular $backup
    if ((Hash $stage) -ne $newHash -or (Hash $destination) -ne $oldHash -or (Hash $backup) -ne $oldHash) {{
        throw 'Update files changed; destination was not replaced'
    }}
    try {{
        [IO.File]::Replace($stage, $destination, $backup, $false)
    }} catch {{
        # ReplaceFile can report a partial filesystem failure. Recover only when
        # the destination vanished; never overwrite an independently changed file.
        if (-not [IO.File]::Exists($destination) -and [IO.File]::Exists($backup) -and (Hash $backup) -eq $oldHash) {{
            $restore = $backup + '.restore'
            [IO.File]::Copy($backup, $restore, $false)
            [IO.File]::Move($restore, $destination)
        }}
        throw
    }}
    if ((Hash $destination) -ne $newHash) {{
        # Both source and old executable remain available for explicit recovery.
        throw 'Post-install digest mismatch; recover from backup before restarting'
    }}
    [IO.File]::WriteAllText($status, 'installed')
    Write-Output 'LightRemote update installed. Restart explicitly when ready.'
}} finally {{
    if ($locked) {{ $lock.Unlock(0, 1) }}
    $lock.Dispose()
}}
}} catch {{
    [IO.File]::WriteAllText($status, ('failed: ' + $_.Exception.Message))
    exit 1
}}
"#
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn private_temp() -> tempfile::TempDir {
        let directory = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700)).unwrap();
        }
        directory
    }

    #[cfg(unix)]
    fn elf() -> Vec<u8> {
        let mut bytes = vec![0; 64];
        bytes[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
        bytes[16..18].copy_from_slice(&3_u16.to_le_bytes());
        bytes[18..20].copy_from_slice(&62_u16.to_le_bytes());
        bytes[20..24].copy_from_slice(&1_u32.to_le_bytes());
        bytes[52..54].copy_from_slice(&64_u16.to_le_bytes());
        bytes
    }

    #[cfg(unix)]
    fn tar_candidate(release: &Release, bytes: &[u8]) -> NamedTempFile {
        let mut download = NamedTempFile::new().unwrap();
        {
            let gzip = flate2::write::GzEncoder::new(
                download.as_file_mut(),
                flate2::Compression::default(),
            );
            let mut archive = tar::Builder::new(gzip);
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            let path = format!(
                "{}-{}-{}/{}",
                release.product.component(),
                release.tag,
                release.target.triple(),
                release.product.binary(release.target)
            );
            archive.append_data(&mut header, path, bytes).unwrap();
            archive.into_inner().unwrap().finish().unwrap();
        }
        download
    }

    #[cfg(unix)]
    #[test]
    fn full_transaction_preserves_old_on_busy_bad_binary_and_commit_failure() {
        let dir = private_temp();
        let destination = dir.path().join("lightremote");
        fs::write(&destination, b"old executable").unwrap();
        let gate = SessionGate::new(&dir.path().join("sessions.lock")).unwrap();
        let release = crate::release::fixture(crate::release::Target::LinuxX64);
        let session = gate.enter().unwrap();
        let result = prepare_and_commit(
            &release,
            tar_candidate(&release, &elf()),
            &destination,
            &gate,
        );
        assert!(matches!(result, Err(Error::Busy)));
        assert_eq!(fs::read(&destination).unwrap(), b"old executable");
        drop(session);
        let result = prepare_and_commit(
            &release,
            tar_candidate(&release, b"#!/bin/sh\nevil"),
            &destination,
            &gate,
        );
        assert!(matches!(result, Err(Error::Binary)));
        assert_eq!(fs::read(&destination).unwrap(), b"old executable");
        let result = prepare_with(
            &release,
            tar_candidate(&release, &elf()),
            &destination,
            &gate,
            |_, _| Err(io::Error::other("simulated atomic rename failure")),
        );
        assert!(matches!(result, Err(Error::Io(_))));
        assert_eq!(fs::read(&destination).unwrap(), b"old executable");
        let result = prepare_and_commit(
            &release,
            tar_candidate(&release, &elf()),
            &destination,
            &gate,
        )
        .unwrap();
        let InstalledUpdate::Installed {
            backup, version, ..
        } = result
        else {
            panic!("expected Unix installed state");
        };
        assert_eq!(version, "0.2.0");
        assert_eq!(fs::read(backup).unwrap(), b"old executable");
        assert_eq!(fs::read(&destination).unwrap(), elf());
        // An old still-running process must not reapply a cached equal/older release.
        let result = prepare_and_commit(
            &release,
            tar_candidate(&release, &elf()),
            &destination,
            &gate,
        );
        assert!(matches!(result, Err(Error::NotUpgrade)));
        assert_eq!(fs::read(&destination).unwrap(), elf());
    }

    #[test]
    fn windows_pending_is_prepared_without_replacing_old_binary() {
        use zip::write::SimpleFileOptions;
        let dir = private_temp();
        let destination = dir.path().join("lightremote.exe");
        fs::write(&destination, b"old executable").unwrap();
        let gate = SessionGate::new(&dir.path().join("sessions.lock")).unwrap();
        let release = crate::release::fixture(crate::release::Target::WindowsX64);
        let mut pe = vec![0; 512];
        pe[..2].copy_from_slice(b"MZ");
        pe[0x3c..0x40].copy_from_slice(&64_u32.to_le_bytes());
        pe[64..68].copy_from_slice(b"PE\0\0");
        pe[68..70].copy_from_slice(&0x8664_u16.to_le_bytes());
        pe[84..86].copy_from_slice(&240_u16.to_le_bytes());
        pe[86..88].copy_from_slice(&2_u16.to_le_bytes());
        pe[88..90].copy_from_slice(&0x20b_u16.to_le_bytes());
        let mut download = NamedTempFile::new().unwrap();
        {
            let mut archive = zip::ZipWriter::new(download.as_file_mut());
            archive
                .start_file(
                    "lightremote-headless-v0.2.0-x86_64-pc-windows-msvc/lightremote.exe",
                    SimpleFileOptions::default(),
                )
                .unwrap();
            archive.write_all(&pe).unwrap();
            archive.finish().unwrap();
        }
        let result = prepare_and_commit(&release, download, &destination, &gate).unwrap();
        let InstalledUpdate::Pending(pending) = result else {
            panic!("expected Windows pending state");
        };
        assert_eq!(fs::read(destination).unwrap(), b"old executable");
        assert_eq!(fs::read(&pending.backup).unwrap(), b"old executable");
        assert_eq!(fs::read(&pending.staged).unwrap(), pe);
        assert!(
            fs::read(&pending.script)
                .unwrap()
                .starts_with(b"\xef\xbb\xbf")
        );
        assert_eq!(
            file_digest(&mut File::open(&pending.script).unwrap()).unwrap(),
            pending.script_digest
        );
        assert!(
            gate.enter().is_ok(),
            "pending preparation must release its gate"
        );
    }

    #[test]
    fn journal_checks_actual_installed_digest_and_never_downgrades() {
        let dir = private_temp();
        let gate = SessionGate::new(&dir.path().join("sessions.lock")).unwrap();
        let _exclusive = gate.exclusive().unwrap();
        let old = "a".repeat(64);
        let new = "b".repeat(64);
        record_version(&gate, &Version::parse("2.0.0").unwrap(), &old, &new).unwrap();
        // A failed installation did not change the old digest, so retry is allowed.
        record_version(&gate, &Version::parse("2.0.0").unwrap(), &old, &new).unwrap();
        assert!(matches!(
            record_version(&gate, &Version::parse("1.5.0").unwrap(), &new, &old),
            Err(Error::NotUpgrade)
        ));
        assert!(matches!(
            record_version(&gate, &Version::parse("2.0.0").unwrap(), &new, &old),
            Err(Error::NotUpgrade)
        ));
        record_version(&gate, &Version::parse("2.0.1").unwrap(), &new, &old).unwrap();
    }

    #[test]
    fn atomic_rename_failure_preserves_existing_executable() {
        let dir = tempfile::tempdir().unwrap();
        let destination = dir.path().join("lightremote");
        fs::write(&destination, b"old working binary").unwrap();
        assert!(atomic_replace(&dir.path().join("missing-stage"), &destination).is_err());
        assert_eq!(fs::read(&destination).unwrap(), b"old working binary");
        let wrong_source = dir.path().join("directory-stage");
        fs::create_dir(&wrong_source).unwrap();
        assert!(atomic_replace(&wrong_source, &destination).is_err());
        assert_eq!(fs::read(destination).unwrap(), b"old working binary");
    }

    #[test]
    fn helper_quotes_paths_and_checks_gate_and_both_digests() {
        let script = windows_script(
            Path::new("C:/a'b/stage"),
            Path::new("C:/a'b/app.exe"),
            Path::new("C:/a'b/backup"),
            Path::new("C:/state/gate"),
            Path::new("C:/state/status"),
            &"a".repeat(64),
            &"b".repeat(64),
        )
        .unwrap();
        assert!(script.contains("'C:/a''b/stage'"));
        let lock = script.find("$lock.Lock(0, 1)").unwrap();
        let verify = script.find("if ((Hash $stage)").unwrap();
        let commit = script.find("[IO.File]::Replace(").unwrap();
        assert!(lock < verify && verify < commit);
        assert!(script.contains("$lock.Unlock(0, 1)"));
        assert!(script.contains("WaitForExit(180000)"));
        assert!(!script.contains("-Verb RunAs"));
        assert!(ps_literal(Path::new("bad\npath")).is_err());
    }
}
