use std::path::PathBuf;

/// A result from checking, preparing, or installing an update.
pub type Result<T> = std::result::Result<T, Error>;

/// Errors callers can distinguish without parsing diagnostic strings.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The running or release version is not strict SemVer.
    #[error("invalid semantic version: {0}")]
    Version(#[from] semver::Error),
    /// Only stable versions with greater SemVer precedence are installable.
    #[error("release is not a stable upgrade")]
    NotUpgrade,
    /// The build's OS or architecture has no published artifact.
    #[error("unsupported target")]
    UnsupportedTarget,
    /// A URL escaped the fixed repository or trusted HTTPS download hosts.
    #[error("untrusted update URL")]
    UntrustedUrl,
    /// The server, transport, status code, or timeout failed.
    #[error("update request failed: {0}")]
    Http(#[from] reqwest::Error),
    /// Release metadata was malformed or incomplete.
    #[error("invalid release metadata: {0}")]
    Metadata(&'static str),
    /// JSON parsing failed.
    #[error("invalid release JSON: {0}")]
    Json(#[from] serde_json::Error),
    /// The advertised or actual size exceeded a configured hard bound.
    #[error("update exceeds {0} byte limit")]
    SizeLimit(u64),
    /// GitHub's SHA-256 or SHA256SUMS was missing, ambiguous, or mismatched.
    #[error("missing or mismatched SHA-256 digest")]
    Digest,
    /// An archive entry was unsafe or did not match the expected product layout.
    #[error("unsafe archive: {0}")]
    Archive(&'static str),
    /// ZIP parsing or decompression failed.
    #[error("invalid zip archive: {0}")]
    Zip(#[from] zip::result::ZipError),
    /// The executable header did not match the target's format or machine.
    #[error("candidate executable format or architecture does not match")]
    Binary,
    /// A session or another installation owns the admission lock.
    #[error("sessions or another update are active")]
    Busy,
    /// The destination or lock path was not an ordinary private-directory file.
    #[error("unsafe installation path: {0}")]
    UnsafePath(PathBuf),
    /// A file operation failed; the old destination is retained until atomic commit.
    #[error("update file operation failed: {0}")]
    Io(#[from] std::io::Error),
    /// PowerShell failed or did not acknowledge startup within 15 seconds.
    #[error("deferred update helper failed to start or exited with code {0:?}")]
    HelperFailed(Option<i32>),
    /// A blocking verification task was interrupted.
    #[error("update worker failed: {0}")]
    Worker(#[from] tokio::task::JoinError),
}
