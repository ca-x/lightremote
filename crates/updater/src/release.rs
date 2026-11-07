use std::{cmp::Ordering, path::Path, time::Duration};

use reqwest::{Client, Url, redirect::Policy};
use semver::Version;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

use crate::{Error, InstalledUpdate, Result, SessionGate, install};

const API: &str = "https://api.github.com/repos/lib-x/lightremote/releases/latest";
const METADATA_LIMIT: u64 = 2 * 1024 * 1024;
const SUMS_LIMIT: u64 = 128 * 1024;
pub(crate) const DOWNLOAD_LIMIT: u64 = 256 * 1024 * 1024;

/// The independently published binary products in the official release matrix.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
#[non_exhaustive]
pub enum Product {
    /// CLI built with capture/input/clipboard support.
    DesktopCli,
    /// CLI built without desktop dependencies.
    HeadlessCli,
    /// NSH native controller.
    Native,
    /// Rendezvous relay.
    Relay,
    /// Resident host service, built independently from the interactive CLI.
    Daemon,
}

impl Product {
    pub(crate) fn component(self) -> &'static str {
        match self {
            Self::DesktopCli => "lightremote",
            Self::HeadlessCli => "lightremote-headless",
            Self::Native => "lightremote-native",
            Self::Relay => "lightremote-relay",
            Self::Daemon => "lightremote-daemon",
        }
    }

    pub(crate) fn binary(self, target: Target) -> String {
        let name = match self {
            Self::HeadlessCli => "lightremote",
            _ => self.component(),
        };
        format!("{name}{}", if target.windows() { ".exe" } else { "" })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Target {
    WindowsX64,
    MacX64,
    MacArm64,
    LinuxX64,
    LinuxArm64,
}

impl Target {
    pub(crate) fn current() -> Result<Self> {
        match (std::env::consts::OS, std::env::consts::ARCH) {
            ("windows", "x86_64") => Ok(Self::WindowsX64),
            ("macos", "x86_64") => Ok(Self::MacX64),
            ("macos", "aarch64") => Ok(Self::MacArm64),
            ("linux", "x86_64") if cfg!(target_env = "gnu") => Ok(Self::LinuxX64),
            ("linux", "aarch64") if cfg!(target_env = "gnu") => Ok(Self::LinuxArm64),
            _ => Err(Error::UnsupportedTarget),
        }
    }

    pub(crate) fn triple(self) -> &'static str {
        match self {
            Self::WindowsX64 => "x86_64-pc-windows-msvc",
            Self::MacX64 => "x86_64-apple-darwin",
            Self::MacArm64 => "aarch64-apple-darwin",
            Self::LinuxX64 => "x86_64-unknown-linux-gnu",
            Self::LinuxArm64 => "aarch64-unknown-linux-gnu",
        }
    }

    pub(crate) fn windows(self) -> bool {
        self == Self::WindowsX64
    }
}

/// An immutable validated stable release, produced only by [`Updater::check`].
#[derive(Clone, Debug)]
pub struct Release {
    pub(crate) version: Version,
    pub(crate) tag: String,
    pub(crate) product: Product,
    pub(crate) target: Target,
    pub(crate) asset: Asset,
    sums: Asset,
    release_url: String,
}

impl Release {
    /// Returns the strict stable version without the tag's `v` prefix.
    #[must_use]
    pub fn version(&self) -> String {
        self.version.to_string()
    }
    /// Returns the exact GitHub release tag used in archive names.
    #[must_use]
    pub fn tag(&self) -> &str {
        &self.tag
    }
    /// Returns the fixed official GitHub release page for display.
    #[must_use]
    pub fn release_url(&self) -> &str {
        &self.release_url
    }
    /// Returns the exact product-specific archive name.
    #[must_use]
    pub fn asset_name(&self) -> &str {
        &self.asset.name
    }
}

/// A reusable client for official GitHub stable releases, with no background tasks.
#[derive(Clone, Debug)]
pub struct Updater {
    client: Client,
    product: Product,
    current: Version,
    target: Target,
}

impl Updater {
    /// Builds a bounded HTTPS client; `current_version` must be strict SemVer.
    ///
    /// # Errors
    /// Rejects invalid versions, unsupported builds, or TLS client setup failure.
    pub fn new(product: Product, current_version: &str) -> Result<Self> {
        let client = Client::builder()
            .https_only(true)
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(180))
            .redirect(Policy::custom(|attempt| {
                if attempt.previous().len() >= 4 {
                    attempt.error("too many update redirects")
                } else if trusted_url(attempt.url()) {
                    attempt.follow()
                } else {
                    attempt.error("untrusted update redirect")
                }
            }))
            .user_agent(concat!("lightremote-updater/", env!("CARGO_PKG_VERSION")))
            .build()?;
        Ok(Self {
            client,
            product,
            current: Version::parse(current_version)?,
            target: Target::current()?,
        })
    }

    /// Checks the official latest release, without installing or executing anything.
    ///
    /// A missing release, draft, prerelease, equal version, or older release returns
    /// `None`. Malformed metadata and incomplete integrity information fail closed.
    /// Callers control scheduling/backoff and should retain the running version until restart.
    ///
    /// # Errors
    /// Reports bounded HTTP, parsing, version, URL, and integrity metadata failures.
    pub async fn check(&self) -> Result<Option<Release>> {
        let response = self
            .client
            .get(API)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .timeout(Duration::from_secs(20))
            .send()
            .await?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let bytes = bounded_body(response.error_for_status()?, METADATA_LIMIT).await?;
        self.select(serde_json::from_slice(&bytes)?)
    }

    /// Downloads, verifies, and installs while exclusively excluding live sessions.
    ///
    /// On Unix the old inode keeps running and the result requires an explicit restart.
    /// Windows returns an auditable deferred helper; no process is restarted here.
    /// `destination` and the gate must be in directories writable only by this user.
    ///
    /// # Errors
    /// Fails before replacement on unsafe downloads, archives, paths, active sessions,
    /// or staging errors. Atomic replacement failure retains the old destination.
    pub async fn install(
        &self,
        release: &Release,
        destination: &Path,
        gate: &SessionGate,
    ) -> Result<InstalledUpdate> {
        ensure_upgrade(&self.current, &release.version)?;
        if release.product != self.product || release.target != self.target {
            return Err(Error::Metadata("release belongs to a different updater"));
        }
        // Fast admission check; commit reacquires and holds the gate after all awaits.
        drop(gate.exclusive()?);
        let sums = self.download_memory(&release.sums, SUMS_LIMIT).await?;
        verify_checksum_asset(&release.asset, &sums)?;
        let download = self.download_archive(&release.asset).await?;
        let release = release.clone();
        let destination = destination.to_owned();
        let gate = gate.clone();
        // This worker is deliberately non-cancellable once commit begins. Dropping the
        // awaiting future cannot interrupt an atomic replacement halfway through.
        tokio::task::spawn_blocking(move || {
            install::prepare_and_commit(&release, download, &destination, &gate)
        })
        .await?
    }

    fn select(&self, metadata: Metadata) -> Result<Option<Release>> {
        if metadata.draft || metadata.prerelease {
            return Ok(None);
        }
        let version = Version::parse(
            metadata
                .tag_name
                .strip_prefix('v')
                .unwrap_or(&metadata.tag_name),
        )?;
        if !version.pre.is_empty() || version.cmp_precedence(&self.current) != Ordering::Greater {
            return Ok(None);
        }
        let root = format!(
            "{}-{}-{}",
            self.product.component(),
            metadata.tag_name,
            self.target.triple()
        );
        let name = format!(
            "{root}{}",
            if self.target.windows() {
                ".zip"
            } else {
                ".tar.gz"
            }
        );
        let asset = select_asset(&metadata.assets, &metadata.tag_name, &name, DOWNLOAD_LIMIT)?;
        let sums = select_asset(
            &metadata.assets,
            &metadata.tag_name,
            "SHA256SUMS",
            SUMS_LIMIT,
        )?;
        let release_url = format!(
            "https://github.com/lib-x/lightremote/releases/tag/{}",
            metadata.tag_name
        );
        Ok(Some(Release {
            version,
            tag: metadata.tag_name,
            product: self.product,
            target: self.target,
            asset,
            sums,
            release_url,
        }))
    }

    async fn download_memory(&self, asset: &Asset, limit: u64) -> Result<Vec<u8>> {
        let bytes = bounded_body(
            self.client
                .get(&asset.browser_download_url)
                .send()
                .await?
                .error_for_status()?,
            limit,
        )
        .await?;
        verify_download(asset, bytes.len() as u64, sha256(&bytes))?;
        Ok(bytes)
    }

    async fn download_archive(&self, asset: &Asset) -> Result<tempfile::NamedTempFile> {
        let mut response = self
            .client
            .get(&asset.browser_download_url)
            .send()
            .await?
            .error_for_status()?;
        if response
            .content_length()
            .is_some_and(|len| len > DOWNLOAD_LIMIT)
        {
            return Err(Error::SizeLimit(DOWNLOAD_LIMIT));
        }
        let temp = tempfile::NamedTempFile::new()?;
        let mut file = tokio::fs::File::from_std(temp.as_file().try_clone()?);
        let mut size = 0_u64;
        let mut hash = Sha256::new();
        while let Some(chunk) = response.chunk().await? {
            size = size
                .checked_add(chunk.len() as u64)
                .ok_or(Error::SizeLimit(DOWNLOAD_LIMIT))?;
            if size > DOWNLOAD_LIMIT || size > asset.size {
                return Err(Error::SizeLimit(DOWNLOAD_LIMIT));
            }
            hash.update(&chunk);
            file.write_all(&chunk).await?;
        }
        file.flush().await?;
        verify_download(asset, size, hash.finalize().into())?;
        Ok(temp)
    }
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct Asset {
    pub(crate) name: String,
    pub(crate) browser_download_url: String,
    pub(crate) size: u64,
    pub(crate) digest: String,
}

#[derive(Debug, Deserialize)]
struct Metadata {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    assets: Vec<Asset>,
}

fn select_asset(assets: &[Asset], tag: &str, name: &str, limit: u64) -> Result<Asset> {
    let mut matching = assets.iter().filter(|asset| asset.name == name);
    let asset = matching
        .next()
        .ok_or(Error::Metadata("required asset is missing"))?;
    if matching.next().is_some() {
        return Err(Error::Metadata("duplicate release asset"));
    }
    let expected = format!("https://github.com/lib-x/lightremote/releases/download/{tag}/{name}");
    if asset.browser_download_url != expected
        || !trusted_url(&Url::parse(&expected).map_err(|_| Error::UntrustedUrl)?)
    {
        return Err(Error::UntrustedUrl);
    }
    if asset.size == 0 || asset.size > limit {
        return Err(Error::SizeLimit(limit));
    }
    parse_digest(&asset.digest)?;
    Ok(asset.clone())
}

fn trusted_url(url: &Url) -> bool {
    if url.scheme() != "https"
        || url.port_or_known_default() != Some(443)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return false;
    }
    match url.host_str() {
        Some("api.github.com") => url.path() == "/repos/lib-x/lightremote/releases/latest",
        Some("github.com") => url
            .path()
            .starts_with("/lib-x/lightremote/releases/download/"),
        Some("release-assets.githubusercontent.com") => {
            url.path().starts_with("/github-production-release-asset/")
        }
        _ => false,
    }
}

async fn bounded_body(mut response: reqwest::Response, limit: u64) -> Result<Vec<u8>> {
    if response.content_length().is_some_and(|len| len > limit) {
        return Err(Error::SizeLimit(limit));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        if (bytes.len() as u64).saturating_add(chunk.len() as u64) > limit {
            return Err(Error::SizeLimit(limit));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

pub(crate) fn ensure_upgrade(current: &Version, next: &Version) -> Result<()> {
    if !next.pre.is_empty() || next.cmp_precedence(current) != Ordering::Greater {
        Err(Error::NotUpgrade)
    } else {
        Ok(())
    }
}

pub(crate) fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

fn verify_download(asset: &Asset, size: u64, digest: [u8; 32]) -> Result<()> {
    if size != asset.size || digest != parse_digest(&asset.digest)? {
        Err(Error::Digest)
    } else {
        Ok(())
    }
}

fn verify_checksum_asset(asset: &Asset, sums: &[u8]) -> Result<()> {
    if checksum(sums, &asset.name)? != parse_digest(&asset.digest)? {
        Err(Error::Digest)
    } else {
        Ok(())
    }
}

fn parse_digest(value: &str) -> Result<[u8; 32]> {
    parse_hex(value.strip_prefix("sha256:").ok_or(Error::Digest)?)
}

fn parse_hex(value: &str) -> Result<[u8; 32]> {
    if value.len() != 64 || !value.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err(Error::Digest);
    }
    let mut digest = [0; 32];
    for (byte, hex) in digest.iter_mut().zip(value.as_bytes().as_chunks::<2>().0) {
        let digit = |c: u8| {
            if c.is_ascii_digit() {
                c - b'0'
            } else {
                c.to_ascii_lowercase() - b'a' + 10
            }
        };
        *byte = digit(hex[0]) * 16 + digit(hex[1]);
    }
    Ok(digest)
}

fn checksum(bytes: &[u8], name: &str) -> Result<[u8; 32]> {
    let text = std::str::from_utf8(bytes).map_err(|_| Error::Digest)?;
    let mut seen = std::collections::HashSet::new();
    let mut found = None;
    for line in text.lines() {
        let (hex, file) = line.split_once("  ").ok_or(Error::Digest)?;
        let hash = parse_hex(hex)?;
        if file.is_empty()
            || file.contains(['/', '\\', ':'])
            || file.chars().any(char::is_whitespace)
            || !seen.insert(file)
        {
            return Err(Error::Digest);
        }
        if file == name {
            found = Some(hash);
        }
    }
    found.ok_or(Error::Digest)
}

#[cfg(test)]
pub(crate) fn fixture(target: Target) -> Release {
    let asset = Asset {
        name: String::new(),
        browser_download_url: String::new(),
        size: 0,
        digest: String::new(),
    };
    Release {
        version: Version::parse("0.2.0").unwrap(),
        tag: "v0.2.0".into(),
        product: Product::HeadlessCli,
        target,
        asset: asset.clone(),
        sums: asset,
        release_url: String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn semver_precedence_rejects_downgrades_pre_and_build_only_updates() {
        let current = Version::parse("1.2.3+build.1").unwrap();
        for value in ["1.2.2", "1.2.3", "1.2.3+build.2", "1.3.0-beta.1"] {
            assert!(
                matches!(
                    ensure_upgrade(&current, &Version::parse(value).unwrap()),
                    Err(Error::NotUpgrade)
                ),
                "{value}"
            );
        }
        assert!(ensure_upgrade(&current, &Version::parse("1.2.4").unwrap()).is_ok());
        for value in ["01.2.3", "v1.2.3", "1.2", "1.2.3garbage"] {
            assert!(Version::parse(value).is_err(), "{value}");
        }
    }

    #[test]
    fn checksums_require_exact_unique_matching_entry() {
        let digest = format!("{:x}", Sha256::digest(b"release"));
        let valid = format!("{digest}  product.tar.gz\n");
        assert_eq!(
            checksum(valid.as_bytes(), "product.tar.gz").unwrap(),
            sha256(b"release")
        );
        assert!(checksum(valid.as_bytes(), "other.tar.gz").is_err());
        assert!(checksum(format!("{valid}{valid}").as_bytes(), "product.tar.gz").is_err());
        assert!(
            checksum(
                format!("{digest}  ../product.tar.gz\n").as_bytes(),
                "product.tar.gz"
            )
            .is_err()
        );
        assert!(parse_digest("sha256:bad").is_err());
        assert!(parse_digest(&format!("sha512:{digest}")).is_err());
    }

    #[test]
    fn corrupted_download_size_or_disagreeing_github_and_sums_are_rejected() {
        let hash = format!("{:x}", Sha256::digest(b"release"));
        let asset = Asset {
            name: "release.tar.gz".into(),
            browser_download_url: String::new(),
            size: 7,
            digest: format!("sha256:{hash}"),
        };
        assert!(verify_download(&asset, 7, sha256(b"release")).is_ok());
        assert!(matches!(
            verify_download(&asset, 7, sha256(b"corrupt")),
            Err(Error::Digest)
        ));
        assert!(matches!(
            verify_download(&asset, 8, sha256(b"release")),
            Err(Error::Digest)
        ));
        let sums = format!("{hash}  release.tar.gz\n");
        assert!(verify_checksum_asset(&asset, sums.as_bytes()).is_ok());
        let wrong = format!("{}  release.tar.gz\n", "0".repeat(64));
        assert!(matches!(
            verify_checksum_asset(&asset, wrong.as_bytes()),
            Err(Error::Digest)
        ));
    }

    #[test]
    fn redirects_never_leave_github_https_allowlist() {
        for url in [
            "http://github.com/lib-x/lightremote/releases/download/v1/a",
            "https://github.com.evil.test/lib-x/lightremote/releases/download/v1/a",
            "https://api.github.com/repos/evil/repo/releases/latest",
            "https://github.com/evil/repo/releases/download/v1/a",
            "https://user@release-assets.githubusercontent.com/github-production-release-asset/a",
            "https://release-assets.githubusercontent.com:444/github-production-release-asset/a",
        ] {
            assert!(!trusted_url(&Url::parse(url).unwrap()), "{url}");
        }
        assert!(trusted_url(&Url::parse("https://release-assets.githubusercontent.com/github-production-release-asset/a?token=test").unwrap()));
    }

    #[test]
    fn release_filtering_and_wrong_or_duplicate_assets_fail_closed() {
        let updater = Updater::new(Product::HeadlessCli, "0.1.1").unwrap();
        let metadata = |tag: &str, draft, prerelease| Metadata {
            tag_name: tag.into(),
            draft,
            prerelease,
            assets: vec![],
        };
        for (tag, draft, pre) in [
            ("v9.0.0", true, false),
            ("v9.0.0", false, true),
            ("v0.1.0", false, false),
            ("v0.1.1", false, false),
            ("v0.2.0-beta.1", false, false),
        ] {
            assert!(updater.select(metadata(tag, draft, pre)).unwrap().is_none());
        }
        assert!(updater.select(metadata("v01.2.3", false, false)).is_err());
        assert!(updater.select(metadata("v0.2.0", false, false)).is_err());
        let asset = Asset {
            name: "SHA256SUMS".into(),
            browser_download_url:
                "https://github.com/lib-x/lightremote/releases/download/v0.2.0/SHA256SUMS".into(),
            size: 3,
            digest: format!("sha256:{}", "0".repeat(64)),
        };
        assert!(select_asset(&[asset.clone(), asset.clone()], "v0.2.0", "SHA256SUMS", 10).is_err());
        let mut wrong = asset;
        wrong.browser_download_url = "https://example.com/SHA256SUMS".into();
        assert!(matches!(
            select_asset(&[wrong], "v0.2.0", "SHA256SUMS", 10),
            Err(Error::UntrustedUrl)
        ));
    }
}
