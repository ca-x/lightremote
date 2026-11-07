use crate::security;
use anyhow::{Context, Result, ensure};
use lightremote::{host::Options, update::RestartRequired};
use lightremote_core::{Invite, link::LinkOptions};
use lightremote_services::HostConfig;
use lightremote_updater::Product;
use serde::Deserialize;
use std::{
    fs,
    io::Read,
    path::{Path, PathBuf},
    time::Duration,
};

const CONFIG_LIMIT: u64 = 64 * 1024;
pub const RESTART_EXIT_CODE: u8 = 75;

// Intentionally no Debug: this object contains an operator credential.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    relay: String,
    web_url: String,
    state_dir: PathBuf,
    #[serde(default)]
    relay_token: Option<String>,
    #[serde(default)]
    policy_confirmed: bool,
    #[serde(default)]
    auto_update: bool,
    #[serde(default)]
    network: Network,
    #[serde(default)]
    share_dir: Option<PathBuf>,
    #[serde(default)]
    allow_write: bool,
    #[serde(default)]
    allow_shell: bool,
    #[serde(default)]
    allow_desktop: bool,
    #[serde(default)]
    allow_input: bool,
    #[serde(default)]
    allow_clipboard: bool,
    #[serde(default)]
    allow_clipboard_read: bool,
    #[serde(default)]
    allow_clipboard_write: bool,
    #[serde(default)]
    shell: Option<PathBuf>,
    #[serde(default = "file_limit")]
    max_file_size: u64,
    #[serde(default = "stream_limit")]
    max_concurrent_streams: usize,
    #[serde(default = "io_timeout")]
    io_timeout_secs: u64,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Network {
    #[default]
    Default,
    Slow,
}
fn file_limit() -> u64 {
    HostConfig::default().max_file_size
}
fn stream_limit() -> usize {
    HostConfig::default().max_concurrent_streams
}
fn io_timeout() -> u64 {
    HostConfig::default().io_timeout.as_secs()
}

pub fn load(path: &Path, service: bool) -> Result<Config> {
    security::absolute(path)?;
    security::ancestors(
        path.parent()
            .context("configuration needs a parent directory")?,
    )?;
    let before = security::private(path, false)?;
    ensure!(before.len() <= CONFIG_LIMIT, "configuration exceeds 64 KiB");
    let file = fs::File::open(path).context("cannot open configuration")?;
    let opened = file
        .metadata()
        .context("cannot inspect configuration handle")?;
    ensure!(opened.is_file(), "configuration is not a regular file");
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        ensure!(
            before.dev() == opened.dev() && before.ino() == opened.ino(),
            "configuration changed while opening"
        );
    }
    security::private(path, false)?;
    let mut bytes = Vec::new();
    file.take(CONFIG_LIMIT + 1)
        .read_to_end(&mut bytes)
        .context("cannot read configuration")?;
    ensure!(
        bytes.len() <= CONFIG_LIMIT as usize,
        "configuration exceeds 64 KiB"
    );
    // serde errors can quote user-supplied values, including credentials.
    let mut config: Config = serde_json::from_slice(&bytes)
        .map_err(|_| anyhow::anyhow!("invalid configuration JSON (unknown fields, duplicate fields and wrong types are rejected)"))?;
    if let Some(token) = std::env::var_os("LIGHTREMOTE_RELAY_TOKEN") {
        config.relay_token = Some(
            token
                .into_string()
                .map_err(|_| anyhow::anyhow!("relay token must be UTF-8"))?,
        );
    }
    config.validate(service)?;
    Ok(config)
}

impl Config {
    fn validate(&self, service: bool) -> Result<()> {
        ensure!(
            self.policy_confirmed,
            "unattended daemon requires explicit policy_confirmed=true"
        );
        let token = self.relay_token.as_deref().unwrap_or_default();
        ensure!(
            !token.is_empty()
                && token.len() <= 4096
                && token.bytes().all(|byte| byte.is_ascii_graphic()),
            "a nonempty printable relay token of at most 4096 bytes is required"
        );
        let probe = Invite {
            relay: self.relay.clone(),
            room_id: "probe".into(),
            client_token: "probe".into(),
            secret: "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA".into(),
        };
        probe.url(&self.web_url).map_err(|_| anyhow::anyhow!("invalid relay or client URL; remote endpoints require TLS and no embedded credentials"))?;
        security::absolute(&self.state_dir)?;
        let parent = self
            .state_dir
            .parent()
            .context("state directory needs a parent")?;
        security::ancestors(parent)?;
        match fs::symlink_metadata(&self.state_dir) {
            Ok(_) => {
                security::private(&self.state_dir, true)?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => anyhow::bail!("cannot inspect state directory"),
        }
        self.check_invite()?;
        if let Some(path) = &self.share_dir {
            security::absolute(path)?;
            ensure!(
                fs::metadata(path).is_ok_and(|metadata| metadata.is_dir()),
                "share_dir must be an existing directory"
            );
        }
        if let Some(path) = &self.shell {
            security::absolute(path)?;
            ensure!(
                fs::metadata(path).is_ok_and(|metadata| metadata.is_file()),
                "shell must be an existing executable file"
            );
        }
        ensure!(
            !self.allow_write || self.share_dir.is_some(),
            "allow_write requires share_dir"
        );
        ensure!(
            !self.allow_input || self.allow_desktop,
            "allow_input requires allow_desktop"
        );
        let desktop = self.allow_desktop
            || self.allow_input
            || self.allow_clipboard
            || self.allow_clipboard_read
            || self.allow_clipboard_write;
        ensure!(
            !self.auto_update || !desktop,
            "automatic daemon updates require headless policy; official daemon releases do not include desktop support"
        );
        ensure!(
            cfg!(feature = "desktop") || !desktop,
            "desktop permissions require a desktop-enabled build"
        );
        ensure!(
            !service || !desktop,
            "Windows SCM session 0 cannot provide desktop/input/clipboard; run CLI host in the logged-in user session"
        );
        ensure!(self.max_file_size > 0, "max_file_size must be positive");
        ensure!(
            (1..=64).contains(&self.max_concurrent_streams),
            "max_concurrent_streams must be 1..=64"
        );
        ensure!(
            (1..=300).contains(&self.io_timeout_secs),
            "io_timeout_secs must be 1..=300"
        );
        Ok(())
    }

    fn check_invite(&self) -> Result<()> {
        match fs::symlink_metadata(self.state_dir.join("invite")) {
            Ok(_) => {
                security::private(&self.state_dir.join("invite"), false)?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => anyhow::bail!("cannot inspect invitation file"),
        }
        Ok(())
    }

    pub fn prepare(self) -> Result<Options> {
        if !self.state_dir.exists() {
            let builder = fs::DirBuilder::new();
            #[cfg(unix)]
            let builder = {
                use std::os::unix::fs::DirBuilderExt;
                let mut builder = builder;
                builder.mode(0o700);
                builder
            };
            builder
                .create(&self.state_dir)
                .context("cannot create private state directory")?;
        }
        security::ancestors(
            self.state_dir
                .parent()
                .context("state directory needs a parent")?,
        )?;
        security::private(&self.state_dir, true)?;
        self.check_invite()?;
        let config = HostConfig {
            share_dir: self.share_dir,
            allow_write: self.allow_write,
            allow_shell: self.allow_shell,
            allow_desktop: self.allow_desktop,
            allow_input: self.allow_input,
            allow_clipboard: self.allow_clipboard,
            allow_clipboard_read: self.allow_clipboard_read,
            allow_clipboard_write: self.allow_clipboard_write,
            shell: self.shell,
            max_file_size: self.max_file_size,
            max_concurrent_streams: self.max_concurrent_streams,
            io_timeout: Duration::from_secs(self.io_timeout_secs),
        };
        let mut options = Options::default();
        options.relay = self.relay;
        options.operator_token = self.relay_token.context("relay token is required")?;
        options.web_url = self.web_url;
        options.config = config;
        options.unattended = true;
        options.stay = true;
        options.invite_file = Some(self.state_dir.join("invite"));
        options.session_gate_path = Some(self.state_dir.join("sessions.lock"));
        options.auto_update = self.auto_update;
        options.update_product = Product::Daemon;
        options.network = network_options(self.network);
        Ok(options)
    }
}

fn network_options(network: Network) -> LinkOptions {
    let mut options = LinkOptions::default();
    if matches!(network, Network::Slow) {
        options.connect_timeout = Duration::from_secs(180);
        options.open_timeout = Duration::from_secs(60);
        options.write_timeout = Duration::from_secs(60);
        options.heartbeat_interval = Duration::from_secs(20);
        options.heartbeat_timeout = Duration::from_secs(90);
        options.degraded_after = Duration::from_secs(10);
        options.drain_timeout = Duration::from_secs(120);
    }
    options
}

pub fn exit_code(result: &Result<()>) -> u8 {
    match result {
        Ok(()) => 0,
        Err(error) if error.downcast_ref::<RestartRequired>().is_some() => RESTART_EXIT_CODE,
        Err(_) => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn config(directory: &Path) -> Config {
        serde_json::from_value(json!({
            "relay": "http://127.0.0.1:8787", "web_url": "http://127.0.0.1:5173/",
            "state_dir": directory.join("state"), "relay_token": "private-test-token"
        }))
        .unwrap()
    }

    #[test]
    fn unattended_policy_requires_explicit_confirmation_and_permissions_default_off() {
        let directory = tempfile::tempdir().unwrap();
        let mut config = config(directory.path());
        assert!(config.validate(false).is_err());
        config.policy_confirmed = true;
        config.validate(false).unwrap();
        let options = config.prepare().unwrap();
        assert!(options.unattended && options.stay);
        assert_eq!(
            options.invite_file.unwrap(),
            directory.path().join("state/invite")
        );
        assert_eq!(options.update_product, Product::Daemon);
        let policy = options.config;
        assert!(policy.share_dir.is_none());
        assert!(!policy.allow_write && !policy.allow_shell && !policy.allow_desktop);
        assert!(!policy.allow_input && !policy.allow_clipboard);
        assert!(!policy.allow_clipboard_read && !policy.allow_clipboard_write);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(directory.path().join("state"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
        }
    }

    #[test]
    fn malformed_unknown_duplicate_and_secret_values_do_not_escape_errors() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.json");
        for value in [
            r#"{"relay_token":"secret", "unexpected":"secret"}"#,
            r#"{"relay":"one","relay":"two"}"#,
            r#"{"relay_token": ["secret"]}"#,
        ] {
            fs::write(&path, value).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
            }
            let error = load(&path, false).err().unwrap();
            assert!(!format!("{error:#}").contains("secret"));
        }
    }

    #[cfg(unix)]
    #[test]
    fn rejects_public_files_links_public_state_and_oversize_config() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.json");
        fs::write(&path, b"{}").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o666)).unwrap();
        assert!(load(&path, false).is_err());
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        let link = directory.path().join("link.json");
        symlink(&path, &link).unwrap();
        assert!(load(&link, false).is_err());
        fs::write(&path, vec![b' '; CONFIG_LIMIT as usize + 1]).unwrap();
        assert!(load(&path, false).is_err());
        let mut config = config(directory.path());
        config.policy_confirmed = true;
        fs::create_dir(&config.state_dir).unwrap();
        fs::set_permissions(&config.state_dir, fs::Permissions::from_mode(0o777)).unwrap();
        assert!(config.validate(false).is_err());
        fs::remove_dir(&config.state_dir).unwrap();
        symlink(directory.path(), &config.state_dir).unwrap();
        assert!(config.validate(false).is_err());
    }

    #[test]
    fn policy_paths_limits_and_service_desktop_are_validated_before_run() {
        let directory = tempfile::tempdir().unwrap();
        let mut config = config(directory.path());
        config.policy_confirmed = true;
        config.allow_shell = true;
        config.validate(true).unwrap();
        config.allow_clipboard_read = true;
        assert!(config.validate(true).is_err());
        config.allow_clipboard_read = false;
        config.state_dir = PathBuf::from("relative");
        assert!(config.validate(false).is_err());
        config.state_dir = directory.path().join("../state");
        assert!(config.validate(false).is_err());
        config.state_dir = directory.path().join("state");
        config.max_concurrent_streams = 65;
        assert!(config.validate(false).is_err());
    }

    #[test]
    fn update_restart_is_distinct_from_clean_shutdown_and_failure() {
        assert_eq!(exit_code(&Ok(())), 0);
        assert_eq!(exit_code(&Err(anyhow::anyhow!("failure"))), 1);
        assert_eq!(exit_code(&Err(RestartRequired.into())), 75);
        assert_eq!(
            exit_code(&Err(
                anyhow::Error::new(RestartRequired).context("host stopped")
            )),
            75
        );
    }

    #[test]
    fn automatic_updates_reject_every_desktop_permission_before_feature_checks() {
        let directory = tempfile::tempdir().unwrap();
        for field in [
            "allow_desktop",
            "allow_input",
            "allow_clipboard",
            "allow_clipboard_read",
            "allow_clipboard_write",
        ] {
            let mut value = json!({
                "relay": "http://127.0.0.1:8787", "web_url": "http://127.0.0.1:5173/",
                "state_dir": directory.path().join("state"), "relay_token": "private-test-token",
                "policy_confirmed": true, "auto_update": true,
            });
            value[field] = json!(true);
            if field == "allow_input" {
                value["allow_desktop"] = json!(true);
            }
            let config: Config = serde_json::from_value(value).unwrap();
            let error = config.validate(false).unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains("automatic daemon updates require headless policy"),
                "{field}: {error}"
            );
            assert!(!directory.path().join("state").exists());
        }
    }

    #[test]
    fn headless_policy_allows_automatic_updates_without_creating_state() {
        let directory = tempfile::tempdir().unwrap();
        let mut config = config(directory.path());
        config.policy_confirmed = true;
        config.auto_update = true;
        config.allow_shell = true;
        config.validate(false).unwrap();
        config.validate(true).unwrap();
        assert!(!directory.path().join("state").exists());
    }

    #[test]
    fn slow_network_has_bounded_extended_budgets() {
        let normal = network_options(Network::Default);
        let slow = network_options(Network::Slow);
        assert!(slow.heartbeat_timeout > normal.heartbeat_timeout);
        assert!(slow.degraded_after <= slow.heartbeat_timeout);
        assert!(slow.connect_timeout <= Duration::from_secs(300));
    }
}
