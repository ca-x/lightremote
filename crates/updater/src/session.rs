use std::{
    env,
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
};

use sha2::{Digest, Sha256};

use crate::{Error, Result};

/// A cross-process admission gate shared by every session and update of one install.
///
/// Prefer [`Self::for_executable`] so CLI, native, and deferred helpers agree on a
/// private writable lock directory, even when the executable directory is read-only.
/// Never unlink, rename, or replace the lock file. All participants must cooperate.
#[derive(Clone, Debug)]
pub struct SessionGate {
    path: PathBuf,
}

/// A live shared lock. Retain it until the corresponding session fully shuts down.
#[derive(Debug)]
pub struct SessionLease {
    _file: File,
}

impl SessionGate {
    /// Derives a stable gate in the current user's writable private state directory.
    ///
    /// Uses SHA-256 of the canonical executable path, without depending on binary
    /// contents. Linux: `$XDG_STATE_HOME/lightremote/update-locks` (or
    /// `$HOME/.local/state/lightremote/update-locks`); macOS:
    /// `$HOME/Library/Application Support/LightRemote/update-locks`; Windows:
    /// `$LOCALAPPDATA/LightRemote/update-locks`. Every participant must use the same
    /// user/environment. Unix leaf directories must be owned by the user and `0700`.
    ///
    /// # Errors
    /// Rejects symlinks in the state path, unsafe permissions/ownership, missing
    /// executable or environment, unsupported OS, and filesystem failures.
    pub fn for_executable(executable: &Path) -> Result<Self> {
        let executable = fs::canonicalize(executable)?;
        if !fs::metadata(&executable)?.is_file() {
            return Err(Error::UnsafePath(executable));
        }
        let directory = state_directory()?;
        private_directory(&directory)?;
        let mut hash = Sha256::new();
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            hash.update(executable.as_os_str().as_bytes());
        }
        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStrExt;
            for unit in executable.as_os_str().encode_wide() {
                hash.update(unit.to_le_bytes());
            }
        }
        #[cfg(not(any(unix, windows)))]
        hash.update(executable.to_string_lossy().as_bytes());
        Self::new(&directory.join(format!("{:x}.lock", hash.finalize())))
    }

    /// Opens or creates a persistent lock in an existing private directory.
    ///
    /// `for_executable` is the shared default. Use this override only when every
    /// participating process is configured with the same private lock path.
    ///
    /// # Errors
    /// Returns an I/O error or rejects symlinks, non-files and unsafe Unix permissions.
    pub fn new(path: &Path) -> Result<Self> {
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        validate_directory(parent)?;
        let name = path
            .file_name()
            .ok_or_else(|| Error::UnsafePath(path.to_owned()))?;
        let gate = Self {
            path: fs::canonicalize(parent)?.join(name),
        };
        gate.open()?;
        Ok(gate)
    }

    /// Returns the canonical persistent lock path used by the Windows deferred helper.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Acquires admission before any session attempt, side effect, or handshake.
    ///
    /// Retain the returned lease through all worker teardown. Do not interpret a
    /// failed acquisition as permission to begin a session without admission.
    ///
    /// # Errors
    /// Returns [`Error::Busy`] while an installation holds the exclusive gate.
    pub fn enter(&self) -> Result<SessionLease> {
        let file = self.open()?;
        fs2::FileExt::try_lock_shared(&file).map_err(lock_error)?;
        Ok(SessionLease { _file: file })
    }

    pub(crate) fn exclusive(&self) -> Result<File> {
        let file = self.open()?;
        fs2::FileExt::try_lock_exclusive(&file).map_err(lock_error)?;
        Ok(file)
    }

    pub(crate) fn journal(&self) -> Result<File> {
        Self {
            path: self.path.with_extension("versions"),
        }
        .open()
    }

    fn open(&self) -> Result<File> {
        if let Ok(metadata) = fs::symlink_metadata(&self.path)
            && (!metadata.is_file() || metadata.file_type().is_symlink())
        {
            return Err(Error::UnsafePath(self.path.clone()));
        }
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
        }
        let file = options.open(&self.path)?;
        let metadata = file.metadata()?;
        if !metadata.is_file() {
            return Err(Error::UnsafePath(self.path.clone()));
        }
        validate_private(&metadata, &self.path, 0o600)?;
        Ok(file)
    }
}

fn state_directory() -> Result<PathBuf> {
    let required = |name: &str| -> Result<PathBuf> {
        let value = env::var_os(name)
            .map(PathBuf::from)
            .ok_or(Error::Metadata("user state directory is unavailable"))?;
        if !value.is_absolute() {
            return Err(Error::UnsafePath(value));
        }
        Ok(value)
    };
    match std::env::consts::OS {
        "linux" => {
            let base = match env::var_os("XDG_STATE_HOME").filter(|s| !s.is_empty()) {
                Some(value) => {
                    let path = PathBuf::from(value);
                    if !path.is_absolute() {
                        return Err(Error::UnsafePath(path));
                    }
                    path
                }
                None => required("HOME")?.join(".local/state"),
            };
            Ok(base.join("lightremote/update-locks"))
        }
        "macos" => {
            Ok(required("HOME")?.join("Library/Application Support/LightRemote/update-locks"))
        }
        "windows" => Ok(required("LOCALAPPDATA")?.join("LightRemote/update-locks")),
        _ => Err(Error::UnsupportedTarget),
    }
}

fn private_directory(path: &Path) -> Result<()> {
    // Validate each existing component before creating children. The leaf is always
    // private; shared system ancestors may remain searchable, never symlinked.
    let mut prefix = PathBuf::new();
    for component in path.components() {
        prefix.push(component);
        match fs::symlink_metadata(&prefix) {
            Ok(metadata) => {
                if !metadata.is_dir() || metadata.file_type().is_symlink() {
                    return Err(Error::UnsafePath(prefix));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let builder = fs::DirBuilder::new();
                #[cfg(unix)]
                let builder = {
                    use std::os::unix::fs::DirBuilderExt;
                    let mut builder = builder;
                    builder.mode(0o700);
                    builder
                };
                match builder.create(&prefix) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(error) => return Err(error.into()),
                }
                let metadata = fs::symlink_metadata(&prefix)?;
                if !metadata.is_dir() || metadata.file_type().is_symlink() {
                    return Err(Error::UnsafePath(prefix));
                }
            }
            Err(error) => return Err(error.into()),
        }
    }
    validate_directory(path)
}

fn validate_directory(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(Error::UnsafePath(path.to_owned()));
    }
    validate_private(&metadata, path, 0o700)
}

fn validate_private(metadata: &fs::Metadata, path: &Path, mode: u32) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        // SAFETY: geteuid takes no pointers and has no preconditions or side effects.
        let user = unsafe { libc::geteuid() };
        if metadata.uid() != user
            || metadata.mode() & 0o7777 != mode
            || (metadata.is_file() && metadata.nlink() != 1)
        {
            return Err(Error::UnsafePath(path.to_owned()));
        }
    }
    #[cfg(not(unix))]
    let _ = (metadata, path, mode);
    Ok(())
}

fn lock_error(error: std::io::Error) -> Error {
    if error.kind() == std::io::ErrorKind::WouldBlock || error.raw_os_error() == Some(33) {
        Error::Busy
    } else {
        Error::Io(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sessions_and_install_exclude_each_other_across_handles() {
        let dir = tempfile::tempdir().unwrap();
        private_directory(&dir.path().join("private")).unwrap();
        let gate = SessionGate::new(&dir.path().join("private/sessions.lock")).unwrap();
        let other = SessionGate::new(gate.path()).unwrap();
        let session = gate.enter().unwrap();
        assert!(matches!(other.exclusive(), Err(Error::Busy)));
        let second = other.enter().unwrap();
        drop((session, second));
        let installation = other.exclusive().unwrap();
        assert!(matches!(gate.enter(), Err(Error::Busy)));
        drop(installation);
        assert!(gate.enter().is_ok());
        assert!(gate.path().exists(), "lock must never be unlinked");
    }

    #[cfg(unix)]
    #[test]
    fn refuses_symlinks_and_permissive_lock_files() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lock");
        fs::write(&path, b"").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(matches!(SessionGate::new(&path), Err(Error::UnsafePath(_))));
        let link = dir.path().join("link");
        symlink(&path, &link).unwrap();
        assert!(matches!(SessionGate::new(&link), Err(Error::UnsafePath(_))));
        let public = dir.path().join("public");
        fs::create_dir(&public).unwrap();
        fs::set_permissions(&public, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(matches!(
            SessionGate::new(&public.join("lock")),
            Err(Error::UnsafePath(_))
        ));
    }

    #[test]
    fn private_state_directory_is_created_and_reused() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("nested/update-locks");
        private_directory(&state).unwrap();
        private_directory(&state).unwrap();
        let gate = SessionGate::new(&state.join("install.lock")).unwrap();
        assert!(gate.enter().is_ok());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&state).unwrap().permissions().mode() & 0o777,
                0o700
            );
            assert_eq!(
                fs::metadata(gate.path()).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }
}
