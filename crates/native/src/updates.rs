//! Resident update checks and explicit, idle-only installation.
use crate::client::State;
use anyhow::{Context, Result, ensure};
use lightremote_updater::{InstalledUpdate, PendingUpdate, Product, Release, SessionGate, Updater};
use std::{
    fs::{self, File, Metadata},
    io::{Read, Write},
    path::PathBuf,
    time::Duration,
};
use tokio::{
    sync::{mpsc, watch},
    task::JoinSet,
};

const CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

const MAX_PREFERENCE_BYTES: u64 = 1024;

/// The gate filename already identifies the canonical install independently of its version.
struct Preferences {
    path: PathBuf,
    #[cfg(unix)]
    owner: u32,
}

impl Preferences {
    fn new(gate: &SessionGate) -> Result<Self> {
        let gate_metadata = fs::symlink_metadata(gate.path()).context("Inspect update gate")?;
        ensure!(
            gate_metadata.is_file() && !is_link(&gate_metadata),
            "Unsafe update gate"
        );
        let preferences = Self {
            path: gate.path().with_extension("native-preferences.json"),
            #[cfg(unix)]
            owner: {
                use std::os::unix::fs::MetadataExt;
                gate_metadata.uid()
            },
        };
        preferences.validate_directory()?;
        preferences.validate_private(&gate_metadata, 0o600)?;
        Ok(preferences)
    }

    fn validate_directory(&self) -> Result<()> {
        let directory = self
            .path
            .parent()
            .context("Preference directory is missing")?;
        for ancestor in directory.ancestors() {
            let metadata =
                fs::symlink_metadata(ancestor).context("Inspect preference directory")?;
            ensure!(
                metadata.is_dir() && !is_link(&metadata),
                "Unsafe preference directory"
            );
        }
        self.validate_private(&fs::symlink_metadata(directory)?, 0o700)
    }

    fn validate_private(&self, metadata: &Metadata, mode: u32) -> Result<()> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            ensure!(
                metadata.uid() == self.owner && metadata.mode() & 0o7777 == mode,
                "Update preferences must be private to the installing user"
            );
            ensure!(
                !metadata.is_file() || metadata.nlink() == 1,
                "Hard-linked update preferences are unsafe"
            );
        }
        #[cfg(not(unix))]
        let _ = (metadata, mode); // The user state directory inherits the gate's platform access controls.
        Ok(())
    }

    fn existing(&self) -> Result<Option<Metadata>> {
        self.validate_directory()?;
        let metadata = match fs::symlink_metadata(&self.path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error).context("Inspect update preferences"),
        };
        ensure!(
            metadata.is_file() && !is_link(&metadata),
            "Update preferences must be a regular file, never a symlink"
        );
        self.validate_private(&metadata, 0o600)?;
        ensure!(
            metadata.len() <= MAX_PREFERENCE_BYTES,
            "Update preferences exceed 1 KiB"
        );
        Ok(Some(metadata))
    }

    fn load(&self) -> Result<Option<bool>> {
        let Some(before) = self.existing()? else {
            return Ok(None);
        };
        let file = File::open(&self.path).context("Open update preferences")?;
        let opened = file.metadata()?;
        ensure!(
            same_file(&before, &opened),
            "Update preferences changed while opening"
        );
        ensure!(
            opened.is_file(),
            "Update preferences are not a regular file"
        );
        self.validate_private(&opened, 0o600)?;
        let mut bytes = Vec::new();
        file.take(MAX_PREFERENCE_BYTES + 1)
            .read_to_end(&mut bytes)?;
        ensure!(
            bytes.len() as u64 <= MAX_PREFERENCE_BYTES,
            "Update preferences exceed 1 KiB"
        );
        let after = self
            .existing()?
            .context("Update preferences disappeared while reading")?;
        ensure!(
            same_file(&opened, &after),
            "Update preferences changed while reading"
        );
        let value: serde_json::Value =
            serde_json::from_slice(&bytes).context("Invalid preference JSON")?;
        let object = value
            .as_object()
            .context("Update preferences must be a JSON object")?;
        ensure!(object.len() == 1, "Unknown update preference fields");
        let automatic = object
            .get("automatic")
            .and_then(serde_json::Value::as_bool)
            .context("The automatic update preference must be a boolean")?;
        Ok(Some(automatic))
    }

    fn save(&self, automatic: bool) -> Result<()> {
        let before = self.existing()?;
        let directory = self
            .path
            .parent()
            .context("Preference directory is missing")?;
        let mut temporary = tempfile::Builder::new()
            .prefix(".native-preferences-")
            .tempfile_in(directory)
            .context("Create private preference temporary file")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            temporary
                .as_file()
                .set_permissions(fs::Permissions::from_mode(0o600))?;
        }
        serde_json::to_writer(&mut temporary, &serde_json::json!({"automatic":automatic}))?;
        temporary.flush()?;
        temporary.as_file().sync_all()?;
        let after = self.existing()?;
        ensure!(
            match (&before, &after) {
                (None, None) => true,
                (Some(before), Some(after)) => same_file(before, after),
                _ => false,
            },
            "Update preferences changed before saving"
        );
        if before.is_some() {
            temporary
                .persist(&self.path)
                .context("Atomically replace update preferences")?;
        } else {
            temporary
                .persist_noclobber(&self.path)
                .context("Publish update preferences without overwriting another file")?;
        }
        // Rename is the commit point; a directory sync error must not claim the old file survived.
        let _ = File::open(directory).and_then(|directory| directory.sync_all());
        Ok(())
    }
}

fn is_link(metadata: &Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_type().is_symlink() || metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

fn same_file(left: &Metadata, right: &Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        left.dev() == right.dev() && left.ino() == right.ino()
    }
    #[cfg(not(unix))]
    {
        left.len() == right.len()
            && left.created().ok() == right.created().ok()
            && left.modified().ok() == right.modified().ok()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpdateState {
    pub automatic: bool,
    pub checking: bool,
    pub installing: bool,
    pub available: Option<String>,
    pub release_url: String,
    pub message: String,
    pub restart_required: bool,
    pub deferred_ready: bool,
}

impl Default for UpdateState {
    fn default() -> Self {
        Self {
            automatic: true,
            checking: false,
            installing: false,
            available: None,
            release_url: String::new(),
            message: format!(
                "Running {} · official GitHub stable releases",
                env!("CARGO_PKG_VERSION")
            ),
            restart_required: false,
            deferred_ready: false,
        }
    }
}

pub enum Command {
    Check,
    Install,
    ApplyDeferred,
    Automatic(bool),
}

enum Finished {
    Check(lightremote_updater::Result<Option<Release>>),
    Install(lightremote_updater::Result<lightremote_updater::InstalledUpdate>),
}

pub async fn run(
    mut commands: mpsc::Receiver<Command>,
    state: watch::Sender<State>,
    gate: SessionGate,
    executable: PathBuf,
) {
    // Load before constructing the interval: a persisted opt-out suppresses its first tick.
    let preferences = Preferences::new(&gate);
    let loaded = preferences
        .as_ref()
        .map_err(|error| format!("{error:#}"))
        .and_then(|preferences| preferences.load().map_err(|error| format!("{error:#}")));
    let mut preference_warning = None;
    state.send_modify(|s| match loaded {
        Ok(Some(enabled)) => s.update.automatic = enabled,
        Ok(None) => {},
        Err(error) => {
            s.update.automatic = false;
            s.update.message = format!("Could not load update preferences: {error}. Automatic checks are off; no preference was changed.");
            preference_warning = Some(s.update.message.clone());
        }
    });
    let updater = match Updater::new(Product::Native, env!("CARGO_PKG_VERSION")) {
        Ok(updater) => updater,
        Err(error) => {
            state.send_modify(|s| s.update.message = format!("Updates unavailable: {error}"));
            return;
        }
    };
    let mut timer = tokio::time::interval(CHECK_INTERVAL);
    timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut tasks = JoinSet::new();
    let mut release: Option<Release> = None;
    let mut deferred: Option<PendingUpdate> = None;
    loop {
        // A background check result must not hide a failure to persist the user's choice.
        if let Some(warning) = &preference_warning {
            state.send_if_modified(|s| {
                if s.update.message.contains(warning) {
                    return false;
                }
                s.update.message.push(' ');
                s.update.message.push_str(warning);
                true
            });
        }
        let command = tokio::select! {
            command = commands.recv() => match command { Some(command) => command, None => break },
            _ = timer.tick() => {
                if !state.borrow().update.automatic { continue; }
                Command::Check
            }
            result = tasks.join_next(), if !tasks.is_empty() => {
                match result {
                    Some(Ok(Finished::Check(result))) => {
                        state.send_modify(|s| {
                            s.update.checking = false;
                            match result {
                                Ok(candidate) => {
                                    s.update.available = candidate.as_ref().map(Release::version);
                                    s.update.release_url = candidate.as_ref().map_or(String::new(), |release| release.release_url().to_owned());
                                    s.update.message = candidate.as_ref().map_or("You have the latest official stable release".into(), |release| format!("Version {} is available from GitHub", release.version()));
                                    release = candidate;
                                }
                                Err(error) => s.update.message = format!("Update check failed: {error}. Retry with Check for updates."),
                            }
                        });
                    }
                    Some(Ok(Finished::Install(result))) => {
                        state.send_modify(|s| {
                            s.update.installing = false;
                            match result {
                                Ok(installed) => {
                                    s.update.restart_required = true;
                                    s.update.available = None;
                                    match installed {
                                        InstalledUpdate::Installed { version, .. } => s.update.message = format!("Version {version} installed. Restart required: quit and reopen LightRemote when ready."),
                                        InstalledUpdate::Pending(transaction) => {
                                            s.update.deferred_ready = true;
                                            s.update.message = format!("Version {} verified and ready. Choose Apply after I quit, then quit and reopen LightRemote to finish. It will not restart automatically.", transaction.version);
                                            deferred = Some(transaction);
                                        }
                                        _ => s.update.message = "Update prepared. Restart required; reopen LightRemote when ready.".into(),
                                    }
                                    release = None;
                                }
                                Err(error) => s.update.message = format!("Update was not installed: {error}"),
                            }
                        });
                    }
                    Some(Err(_)) => state.send_modify(|s| {
                        s.update.checking = false; s.update.installing = false;
                        s.update.message = "Update worker stopped unexpectedly. Check again before installing.".into();
                    }),
                    None => {}
                }
                continue;
            }
        };
        match command {
            Command::ApplyDeferred => {
                let Some(transaction) = deferred.as_ref() else {
                    continue;
                };
                let active = {
                    let current = state.borrow();
                    current.connected || current.connecting
                };
                if active {
                    continue;
                }
                match transaction.launch() {
                    Ok(()) => {
                        deferred = None;
                        state.send_modify(|s| {
                            s.update.deferred_ready = false;
                            s.update.message = "Update helper ready. Quit LightRemote to finish installation, then reopen it yourself. No automatic restart.".into();
                        });
                    }
                    Err(error) => state.send_modify(|s| s.update.message = format!("Could not start the update helper: {error}. You can retry Apply after I quit.")),
                }
            }
            Command::Automatic(enabled) => {
                let saved = preferences
                    .as_ref()
                    .map_err(|error| format!("{error:#}"))
                    .and_then(|preferences| {
                        preferences
                            .save(enabled)
                            .map_err(|error| format!("{error:#}"))
                    });
                state.send_modify(|s| match saved {
                    Ok(()) => {
                        preference_warning = None;
                        s.update.automatic = enabled;
                        s.update.message = format!(
                            "Automatic update checks {}. Preference saved for this installation.",
                            if enabled { "enabled" } else { "disabled" }
                        );
                    }
                    Err(error) => {
                        s.update.message = format!(
                            "Update preference was not saved: {error}. Automatic checks remain {}.",
                            if s.update.automatic { "on" } else { "off" }
                        );
                        preference_warning = Some(s.update.message.clone());
                    }
                });
                // Reset prevents a timer backlog from immediately rechecking on enable.
                timer.reset();
            }
            Command::Check => {
                if !tasks.is_empty() || state.borrow().update.restart_required {
                    continue;
                }
                state.send_modify(|s| {
                    s.update.checking = true;
                    s.update.message = "Checking official GitHub releases…".into();
                });
                let updater = updater.clone();
                tasks.spawn(async move { Finished::Check(updater.check().await) });
            }
            Command::Install => {
                if !tasks.is_empty() || state.borrow().update.restart_required {
                    continue;
                }
                let Some(candidate) = release.clone() else {
                    state.send_modify(|s| {
                        s.update.message = "Check for updates before installing.".into()
                    });
                    continue;
                };
                let mut admitted = false;
                state.send_modify(|s| admitted = s.begin_install());
                if !admitted {
                    continue;
                }
                let updater = updater.clone();
                let gate = gate.clone();
                let executable = executable.clone();
                tasks.spawn(async move {
                    Finished::Install(updater.install(&candidate, &executable, &gate).await)
                });
            }
        }
    }
    tasks.abort_all();
    while tasks.join_next().await.is_some() {}
}

#[cfg(test)]
mod tests {
    use super::*;

    fn private_gate() -> (tempfile::TempDir, SessionGate) {
        let directory = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700)).unwrap();
        }
        let gate = SessionGate::new(&directory.path().join("installation.lock")).unwrap();
        (directory, gate)
    }

    #[test]
    fn preferences_round_trip_per_install_with_a_private_atomic_file() {
        let (directory, gate) = private_gate();
        let preferences = Preferences::new(&gate).unwrap();
        assert_eq!(preferences.load().unwrap(), None);
        assert!(
            !preferences.path.exists(),
            "Loading an absent preference must not create configuration"
        );
        preferences.save(false).unwrap();
        let reopened = Preferences::new(&gate).unwrap();
        assert_eq!(reopened.load().unwrap(), Some(false));
        reopened.save(true).unwrap();
        assert_eq!(preferences.load().unwrap(), Some(true));
        assert_eq!(
            fs::read_to_string(&preferences.path).unwrap(),
            "{\"automatic\":true}"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&preferences.path)
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o7777,
                0o600
            );
        }
        let other_gate =
            SessionGate::new(&directory.path().join("other-installation.lock")).unwrap();
        let other = Preferences::new(&other_gate).unwrap();
        assert_ne!(other.path, preferences.path);
        assert_eq!(other.load().unwrap(), None);
        assert!(fs::read_dir(directory.path()).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".native-preferences-")
        }));
    }

    #[test]
    fn invalid_schema_and_oversized_preferences_are_rejected_without_rewriting() {
        let (_directory, gate) = private_gate();
        let preferences = Preferences::new(&gate).unwrap();
        preferences.save(false).unwrap();
        for invalid in [
            "",
            "not json",
            "null",
            "false",
            "[]",
            "{}",
            "{\"automatic\":null}",
            "{\"automatic\":1}",
            "{\"automatic\":\"false\"}",
            "{\"automatic\":false,\"extra\":true}",
            "{\"automatic\":false} trailing",
        ] {
            fs::write(&preferences.path, invalid).unwrap();
            assert!(
                preferences.load().is_err(),
                "Invalid preference accepted: {invalid}"
            );
            assert_eq!(fs::read_to_string(&preferences.path).unwrap(), invalid);
        }
        let mut boundary = b"{\"automatic\":false}".to_vec();
        boundary.resize(MAX_PREFERENCE_BYTES as usize, b' ');
        fs::write(&preferences.path, &boundary).unwrap();
        assert_eq!(preferences.load().unwrap(), Some(false));
        boundary.push(b' ');
        fs::write(&preferences.path, &boundary).unwrap();
        assert!(preferences.load().is_err());
        assert!(preferences.save(true).is_err());
        assert_eq!(fs::read(&preferences.path).unwrap(), boundary);
    }

    #[test]
    fn a_directory_cannot_be_read_or_replaced_as_a_preference_file() {
        let (_directory, gate) = private_gate();
        let preferences = Preferences::new(&gate).unwrap();
        fs::create_dir(&preferences.path).unwrap();
        assert!(preferences.load().is_err());
        assert!(preferences.save(false).is_err());
        assert!(preferences.path.is_dir());
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_hardlinks_and_nonprivate_paths_never_get_overwritten() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let (directory, gate) = private_gate();
        let preferences = Preferences::new(&gate).unwrap();
        preferences.save(false).unwrap();
        let target = directory.path().join("unrelated.json");
        fs::rename(&preferences.path, &target).unwrap();
        symlink(&target, &preferences.path).unwrap();
        assert!(preferences.load().is_err());
        assert!(preferences.save(true).is_err());
        assert!(
            fs::symlink_metadata(&preferences.path)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            fs::read_to_string(&target).unwrap(),
            "{\"automatic\":false}"
        );
        fs::remove_file(&preferences.path).unwrap();
        fs::hard_link(&target, &preferences.path).unwrap();
        assert!(preferences.load().is_err());
        assert!(preferences.save(true).is_err());
        fs::remove_file(&preferences.path).unwrap();
        fs::rename(&target, &preferences.path).unwrap();
        fs::set_permissions(&preferences.path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(preferences.load().is_err());
        assert!(preferences.save(true).is_err());
        fs::set_permissions(&preferences.path, fs::Permissions::from_mode(0o600)).unwrap();
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o755)).unwrap();
        assert!(preferences.load().is_err());
        assert!(preferences.save(true).is_err());
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(preferences.load().unwrap(), Some(false));
    }

    #[tokio::test]
    async fn persisted_opt_out_is_loaded_before_checks_on_every_restart() {
        let (directory, gate) = private_gate();
        Preferences::new(&gate).unwrap().save(false).unwrap();
        for _ in 0..2 {
            let (state, mut changes) = watch::channel(State::default());
            let (commands, receiver) = mpsc::channel(4);
            let task = tokio::spawn(run(
                receiver,
                state.clone(),
                gate.clone(),
                directory.path().join("unused-executable"),
            ));
            tokio::time::timeout(
                Duration::from_secs(1),
                changes.wait_for(|s| !s.update.automatic),
            )
            .await
            .unwrap()
            .unwrap();
            tokio::task::yield_now().await;
            assert!(!state.borrow().update.checking);
            drop(commands);
            tokio::time::timeout(Duration::from_secs(1), task)
                .await
                .unwrap()
                .unwrap();
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn failed_preference_save_keeps_opt_out_and_explains_that_it_was_not_saved() {
        use std::os::unix::fs::symlink;
        let (directory, gate) = private_gate();
        let preferences = Preferences::new(&gate).unwrap();
        preferences.save(false).unwrap();
        let (state, mut changes) = watch::channel(State::default());
        let (commands, receiver) = mpsc::channel(4);
        let task = tokio::spawn(run(
            receiver,
            state.clone(),
            gate,
            directory.path().join("unused-executable"),
        ));
        tokio::time::timeout(
            Duration::from_secs(1),
            changes.wait_for(|s| !s.update.automatic),
        )
        .await
        .unwrap()
        .unwrap();
        let old = directory.path().join("old-preference.json");
        fs::rename(&preferences.path, &old).unwrap();
        symlink(&old, &preferences.path).unwrap();
        commands.send(Command::Automatic(true)).await.unwrap();
        tokio::time::timeout(
            Duration::from_secs(1),
            changes.wait_for(|s| s.update.message.contains("was not saved")),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(!state.borrow().update.automatic);
        assert!(!state.borrow().update.checking);
        commands.send(Command::Install).await.unwrap();
        tokio::time::timeout(
            Duration::from_secs(1),
            changes.wait_for(|s| {
                s.update.message.starts_with("Check for updates")
                    && s.update.message.contains("was not saved")
            }),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(fs::read_to_string(old).unwrap(), "{\"automatic\":false}");
        drop(commands);
        tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn checks_remain_resident_across_disconnect_and_honor_opt_out() {
        let directory = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
                .unwrap();
        }
        let gate = SessionGate::new(&directory.path().join("sessions.lock")).unwrap();
        Preferences::new(&gate).unwrap().save(false).unwrap();
        let (state, mut changes) = watch::channel(State::default());
        let (commands, receiver) = mpsc::channel(4);
        let task = tokio::spawn(run(
            receiver,
            state.clone(),
            gate,
            directory.path().join("unused-executable"),
        ));
        commands.send(Command::Automatic(false)).await.unwrap();
        tokio::time::timeout(Duration::from_secs(1), changes.changed())
            .await
            .unwrap()
            .unwrap();
        assert!(!state.borrow().update.checking);
        state.send_modify(|s| s.connected = true);
        state.send_modify(|s| s.connected = false);
        commands.send(Command::Automatic(true)).await.unwrap();
        tokio::time::timeout(
            Duration::from_secs(1),
            changes.wait_for(|s| s.update.automatic),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(!task.is_finished());
        assert!(!state.borrow().update.checking);
        commands.send(Command::Automatic(false)).await.unwrap();
        tokio::time::timeout(
            Duration::from_secs(1),
            changes.wait_for(|s| !s.update.automatic),
        )
        .await
        .unwrap()
        .unwrap();
        drop(commands);
        tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap();
    }
}
