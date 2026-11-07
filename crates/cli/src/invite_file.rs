//! Exclusive invitation publication authority, held until host teardown completes.
use anyhow::{Context, Result, bail, ensure};
use std::{
    ffi::OsString,
    fs::{File, Metadata, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

const LOCK_SUFFIX: &str = ".writer.lock";
const INVITE_LIMIT: u64 = 16 * 1024;

/// Closing the file releases the lock; destruction never reads or removes paths.
/// The stable lock file must never be unlinked, including after a clean shutdown.
pub(super) struct InviteFile {
    path: PathBuf,
    lock_path: PathBuf,
    lock: File,
    published: Option<String>,
}

impl InviteFile {
    pub(super) fn acquire(path: &Path) -> Result<Self> {
        let filename = path
            .file_name()
            .context("invitation file needs a filename")?;
        ensure!(
            !filename
                .to_string_lossy()
                .to_ascii_lowercase()
                .ends_with(LOCK_SUFFIX),
            "invitation filename uses the reserved writer lock suffix"
        );
        let parent = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        validate_directory(parent)?;
        // Normalize aliases before deriving the lock name, including relative paths.
        let parent = parent.canonicalize()?;
        validate_directory(&parent)?;
        let path = parent.join(filename);
        validate_optional_file(&path)?;
        let mut lock_name = OsString::from(".");
        lock_name.push(filename);
        lock_name.push(LOCK_SUFFIX);
        let lock_path = parent.join(lock_name);
        let mut options = OpenOptions::new();
        options.read(true).write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let lock = match options.open(&lock_path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                // The private, user-owned parent is trusted. Never truncate an
                // existing lock, and verify its identity again after opening.
                validate_file(&std::fs::symlink_metadata(&lock_path)?)?;
                OpenOptions::new().read(true).write(true).open(&lock_path)?
            }
            Err(error) => return Err(error.into()),
        };
        validate_lock(&lock, &lock_path)?;
        if let Err(error) = fs2::FileExt::try_lock_exclusive(&lock) {
            if error.raw_os_error() == fs2::lock_contended_error().raw_os_error() {
                bail!("another host already owns this invitation file");
            }
            return Err(error).context("acquire invitation writer authority");
        }
        let authority = Self {
            path,
            lock_path,
            lock,
            published: None,
        };
        authority.validate()?;
        Ok(authority)
    }

    fn validate(&self) -> Result<()> {
        validate_directory(
            self.path
                .parent()
                .expect("normalized invitation has a parent"),
        )?;
        validate_lock(&self.lock, &self.lock_path)?;
        validate_optional_file(&self.path)
    }

    pub(super) fn publish(&mut self, invitation: &str) -> Result<()> {
        self.validate()?;
        ensure!(
            invitation.len() < INVITE_LIMIT as usize,
            "invitation exceeds the private file size limit"
        );
        let mut staged = tempfile::NamedTempFile::new_in(
            self.path
                .parent()
                .expect("normalized invitation has a parent"),
        )?;
        writeln!(staged, "{invitation}")?;
        staged.as_file().sync_all()?;
        staged.persist(&self.path).map_err(|error| error.error)?;
        self.published = Some(format!("{invitation}\n"));
        Ok(())
    }

    /// Check replacement and clear explicitly while retaining writer authority.
    pub(super) fn clear(&mut self) -> Result<()> {
        let Some(expected) = &self.published else {
            return Ok(());
        };
        self.validate()?;
        let file = match File::open(&self.path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                self.published = None;
                return Ok(());
            }
            Err(error) => return Err(error.into()),
        };
        let mut bytes = Vec::new();
        file.take(INVITE_LIMIT + 1).read_to_end(&mut bytes)?;
        ensure!(
            bytes == expected.as_bytes(),
            "invitation file was replaced; refusing to clear it"
        );
        // All publishers hold this same persistent lock, including across this
        // comparison and unlink. The private parent is the path trust boundary.
        std::fs::remove_file(&self.path)?;
        self.published = None;
        Ok(())
    }
}

impl Drop for InviteFile {
    fn drop(&mut self) {
        // flock belongs to the open file description, not just this descriptor.
        // End our unique publication authority even if a concurrent fork still
        // holds a copy until exec. Never inspect, replace or unlink the path.
        let _ = fs2::FileExt::unlock(&self.lock);
    }
}

fn validate_directory(path: &Path) -> Result<()> {
    let metadata =
        std::fs::symlink_metadata(path).context("invitation parent directory must exist")?;
    ensure!(
        metadata.is_dir(),
        "invitation parent must be a real directory"
    );
    validate_private(&metadata, "invitation parent directory")
}

fn validate_optional_file(path: &Path) -> Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => validate_file(&metadata),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn validate_file(metadata: &Metadata) -> Result<()> {
    ensure!(
        metadata.is_file(),
        "invitation and lock paths must be regular files"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        ensure!(
            metadata.nlink() == 1,
            "invitation and lock files must not have hard links"
        );
    }
    validate_private(metadata, "invitation or lock file")
}

fn validate_private(metadata: &Metadata, description: &str) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let user = rustix::process::geteuid().as_raw();
        ensure!(
            metadata.uid() == user,
            "{description} must be owned by the current user"
        );
        ensure!(
            metadata.permissions().mode() & 0o7077 == 0,
            "{description} must have private Unix permissions without special mode bits"
        );
    }
    #[cfg(not(unix))]
    let _ = (metadata, description);
    Ok(())
}

fn validate_lock(file: &File, path: &Path) -> Result<()> {
    let opened = file.metadata()?;
    let named = std::fs::symlink_metadata(path)?;
    validate_file(&opened)?;
    validate_file(&named)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        ensure!(
            opened.dev() == named.dev() && opened.ino() == named.ino(),
            "invitation writer lock was replaced"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};

    fn private_directory() -> tempfile::TempDir {
        let directory = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
                .unwrap();
        }
        directory
    }

    #[test]
    fn private_invitation_publication_and_cleanup_are_atomic_and_scoped() {
        let dir = private_directory();
        let path = dir.path().join("invite");
        let mut file = InviteFile::acquire(&path).unwrap();
        file.publish("first").unwrap();
        file.publish("second").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "second\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        let lock_path = file.lock_path.clone();
        file.clear().unwrap();
        assert!(!path.exists());
        assert!(
            InviteFile::acquire(&path).is_err(),
            "clear must retain authority"
        );
        drop(file);
        assert!(lock_path.exists(), "stable lock must never be unlinked");
        let mut next = InviteFile::acquire(&path).unwrap();
        next.publish("third").unwrap();
        next.clear().unwrap();
    }

    #[test]
    fn drop_releases_only_the_lock_and_clear_preserves_replacements() {
        let dir = private_directory();
        let path = dir.path().join("invite");
        let mut file = InviteFile::acquire(&path).unwrap();
        file.publish("first").unwrap();
        std::fs::write(&path, "external replacement\n").unwrap();
        let error = file.clear().unwrap_err();
        assert!(!error.to_string().contains("external replacement"));
        drop(file);
        let mut next = InviteFile::acquire(&path).unwrap();
        next.clear().unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "external replacement\n"
        );
    }

    #[cfg(unix)]
    #[test]
    fn drop_releases_authority_even_while_an_inherited_descriptor_exists() {
        let dir = private_directory();
        let path = dir.path().join("invite");
        let file = InviteFile::acquire(&path).unwrap();
        // A concurrent fork can retain the open file description until exec
        // closes CLOEXEC descriptors, even after the parent's File is dropped.
        let inherited = file.lock.try_clone().unwrap();
        drop(file);
        let _next = InviteFile::acquire(&path)
            .expect("writer authority must end independently of inherited descriptors");
        drop(inherited);
    }

    #[test]
    fn different_invitation_paths_share_session_admission() {
        let dir = private_directory();
        let gate =
            lightremote_updater::SessionGate::new(&dir.path().join("sessions.lock")).unwrap();
        let _first_session = gate.enter().unwrap();
        let _second_session = gate.enter().unwrap();
        let _first = InviteFile::acquire(&dir.path().join("first")).unwrap();
        let _second = InviteFile::acquire(&dir.path().join("second")).unwrap();
    }

    fn writer_command(path: &Path, mode: &str) -> Command {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command
            .args([
                "--exact",
                "host::invite_file::tests::writer_process",
                "--nocapture",
            ])
            .env("LIGHTREMOTE_TEST_INVITE_PATH", path)
            .env("LIGHTREMOTE_TEST_INVITE_MODE", mode);
        command
    }

    #[test]
    fn writer_process() {
        let Some(path) = std::env::var_os("LIGHTREMOTE_TEST_INVITE_PATH") else {
            return;
        };
        if let Some(ready) = std::env::var_os("LIGHTREMOTE_TEST_INVITE_READY") {
            std::fs::write(ready, b"ready").unwrap();
            std::io::stdin().read_exact(&mut [0]).unwrap();
        }
        let authority = InviteFile::acquire(Path::new(&path));
        if std::env::var("LIGHTREMOTE_TEST_INVITE_MODE").unwrap() == "denied" {
            let error = authority
                .err()
                .expect("a live writer must exclude this process");
            assert!(error.to_string().contains("another host already owns"));
        } else {
            authority.unwrap().publish("second process").unwrap();
        }
    }

    #[test]
    fn independent_process_cannot_publish_during_clear_but_can_after_release() {
        let dir = private_directory();
        let path = dir.path().join("invite");
        let ready = dir.path().join("ready");
        let mut first = InviteFile::acquire(&path).unwrap();
        first.publish("first process").unwrap();
        let lock_path = first.lock_path.clone();
        #[cfg(unix)]
        let lock_identity = {
            use std::os::unix::fs::MetadataExt;
            first.lock.metadata().unwrap().ino()
        };
        let mut child = writer_command(&path, "denied")
            .env("LIGHTREMOTE_TEST_INVITE_READY", &ready)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !ready.exists() {
            if std::time::Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("child did not reach the writer barrier");
            }
            assert!(
                child.try_wait().unwrap().is_none(),
                "child exited before barrier"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        child.stdin.take().unwrap().write_all(b"x").unwrap();
        first.clear().unwrap();
        assert!(
            child.wait().unwrap().success(),
            "competing process must be denied"
        );
        assert!(!path.exists());
        assert!(InviteFile::acquire(&path).is_err());
        drop(first);
        assert!(
            writer_command(&path, "publish")
                .output()
                .unwrap()
                .status
                .success()
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "second process\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            assert_eq!(std::fs::metadata(&lock_path).unwrap().ino(), lock_identity);
        }
        assert!(lock_path.exists());
        let _next = InviteFile::acquire(&path).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn invitation_file_rejects_symlinks_and_public_directories() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let dir = private_directory();
        let path = dir.path().join("invite");
        let outside = dir.path().join("outside");
        std::fs::write(&outside, "keep").unwrap();
        symlink(&outside, &path).unwrap();
        assert!(InviteFile::acquire(&path).is_err());
        assert_eq!(std::fs::read_to_string(&outside).unwrap(), "keep");
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(InviteFile::acquire(&dir.path().join("new")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn invitation_and_lock_reject_hardlinks_and_unsafe_modes() {
        use std::os::unix::fs::PermissionsExt;
        let dir = private_directory();
        let path = dir.path().join("invite");
        let file = InviteFile::acquire(&path).unwrap();
        let lock_path = file.lock_path.clone();
        drop(file);
        for target in [&path, &lock_path] {
            std::fs::write(target, b"").unwrap();
            for mode in [0o644, 0o660, 0o4600] {
                std::fs::set_permissions(target, std::fs::Permissions::from_mode(mode)).unwrap();
                assert!(InviteFile::acquire(&path).is_err(), "unsafe mode {mode:o}");
            }
            std::fs::set_permissions(target, std::fs::Permissions::from_mode(0o600)).unwrap();
            let alias = dir.path().join("alias");
            std::fs::hard_link(target, &alias).unwrap();
            assert!(
                InviteFile::acquire(&path).is_err(),
                "hardlink must be rejected"
            );
            std::fs::remove_file(alias).unwrap();
        }
        assert!(InviteFile::acquire(&path).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn lock_symlinks_and_replacement_are_rejected() {
        use std::os::unix::fs::symlink;
        let dir = private_directory();
        let path = dir.path().join("invite");
        let mut file = InviteFile::acquire(&path).unwrap();
        let lock_path = file.lock_path.clone();
        // Simulate an external violation of the persistent-lock protocol.
        std::fs::rename(&lock_path, dir.path().join("old-lock")).unwrap();
        let outside = dir.path().join("outside");
        std::fs::write(&outside, b"keep").unwrap();
        symlink(&outside, &lock_path).unwrap();
        assert!(InviteFile::acquire(&path).is_err());
        assert!(file.publish("secret").is_err());
        assert_eq!(std::fs::read_to_string(&outside).unwrap(), "keep");
        std::fs::remove_file(&lock_path).unwrap();
        let replacement = tempfile::NamedTempFile::new_in(dir.path()).unwrap();
        replacement.persist(&lock_path).unwrap();
        assert!(
            file.publish("secret").is_err(),
            "inode replacement must be detected"
        );
    }

    #[test]
    fn invitation_cannot_replace_another_paths_writer_lock() {
        let dir = private_directory();
        let file = InviteFile::acquire(&dir.path().join("invite")).unwrap();
        assert!(InviteFile::acquire(&file.lock_path).is_err());
    }
}
