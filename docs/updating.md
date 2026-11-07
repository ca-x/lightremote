# Updating LightRemote

`lightremote-updater` checks only the official stable GitHub Release for
[`lib-x/lightremote`](https://github.com/lib-x/lightremote/releases/latest).
There is no separate update server. Checks do not install, restart, terminate a
session, or execute a downloaded binary. The caller owns scheduling, opt-out,
retry/backoff, user consent, and the explicit restart state.

## Integration API

```rust,no_run
use lightremote_updater::{InstalledUpdate, Product, SessionGate, Updater};

# async fn example() -> Result<(), Box<dyn std::error::Error>> {
let executable = std::env::current_exe()?;
let gate = SessionGate::for_executable(&executable)?;
let updater = Updater::new(Product::HeadlessCli, env!("CARGO_PKG_VERSION"))?;

// Acquire BEFORE a session attempt, and retain through ALL worker teardown.
{
    let session_lease = gate.enter()?;
    // Connect, serve the session, cancel/join every worker on disconnect.
    drop(session_lease);
}

// Check periodically, including between sessions in resident operation.
if let Some(release) = updater.check().await? {
    println!("{}: {}", release.version(), release.release_url());
    // After explicit install/auto-update consent, with admission stopped locally:
    match updater.install(&release, &executable, &gate).await? {
        InstalledUpdate::Installed { version, backup, .. } => {
            // Disk has the new executable. This process STILL runs its old version.
            // Enter restart-required state; exit 75 for an appropriate supervisor,
            // or explicitly exec/relaunch after teardown. Never report live upgrade.
            println!("restart required for {version}; backup: {}", backup.display());
        }
        InstalledUpdate::Pending(pending) => {
            // Windows: verified staging, no replacement yet. Stop admitting sessions.
            pending.launch()?;
            // Exit this process after success, so the helper can replace its exe.
            // launch() confirms helper readiness, not successful installation.
        }
        _ => {}
    }
}
# Ok(())
# }
```

Products are `DesktopCli`, `HeadlessCli`, `Native`, `Relay`, and `Daemon`. The headless
archive contains `lightremote[.exe]`, although its component name is
`lightremote-headless`. `Release` is immutable and constructed only by `check`;
its public accessors are `version() -> String`, `tag() -> &str`,
`asset_name() -> &str`, and `release_url() -> &str`.

The exact installation states are:

```text
InstalledUpdate::Installed {
    version: String, destination: PathBuf, backup: PathBuf
}
InstalledUpdate::Pending(PendingUpdate)
PendingUpdate {
    pub version: String, pub destination: PathBuf, pub staged: PathBuf,
    pub backup: PathBuf, pub script: PathBuf, pub status: PathBuf,
    // private integrity state
}
PendingUpdate::launch(&self) -> Result<()>
```

The enum and pending structure are non-exhaustive. `Installed` always requires an
explicit process restart. `Pending` always means replacement has not yet happened.
Keep the original running version when constructing subsequent checks; the crate
also journals verified candidate versions/digests under the gate's exclusive lock,
preventing an old resident process from installing a cached equal/older release
over a destination that it already updated. Journal entries establish a version
only when their digest matches the actual destination. Interrupted preparation
therefore does not block retry. The private journal is bounded to 128 KiB; a full
or malformed journal fails closed and requires operator inspection. Do not clear
it while an old executable is still running.

## Session admission and permissions

`SessionGate::for_executable(&Path) -> Result<SessionGate>` is the shared CLI/native
entry point. It hashes the canonical executable path with SHA-256; changing the
binary's inode or contents does not change its lock path. All processes for an
installation must use the same user and state-directory environment:

| OS | Directory |
| --- | --- |
| Linux | `$XDG_STATE_HOME/lightremote/update-locks`, or `$HOME/.local/state/lightremote/update-locks` |
| macOS | `$HOME/Library/Application Support/LightRemote/update-locks` |
| Windows | `$LOCALAPPDATA/LightRemote/update-locks` |

New Unix directories are `0700`; the leaf directory must be user-owned `0700`.
Lock/journal files must be user-owned, singly linked `0600`. Symlink paths are
rejected; Unix file opening additionally uses `O_NOFOLLOW`. The state tree must not
be controlled by another local user. Windows relies on the user's LocalAppData ACL
inheritance; the crate does not change system ACLs. A custom
`SessionGate::new(&lock_path)` is available for a shared configured private directory.
No lock file is ever unlinked or replaced. `gate.path()` exposes its canonical path.

The independent daemon uses `<state_dir>/sessions.lock`, shared by its host and
updater, instead of a login account's HOME. Its Windows deferred helper receives
that same canonical lock path. Administrator-managed replacement requires stopping
all daemon instances using the installation first; the CLI update command updates
the CLI itself and is not a daemon installer. Preserve the daemon lock across restarts.
Automatic daemon updates require one state directory per executable installation.
For multiple independently configured daemons, use separate executable copies or
disable automatic updates and stop every instance before administrator replacement.
Separate state directories or user accounts do not provide a shared update lock.

`gate.enter()` returns a shared `SessionLease`. Acquire it before attempting a
session or publishing/consuming an invitation, and hold it until all workers stop.
Installation performs an early idle check, then reacquires an exclusive lock after
all download/verification awaits and retains it through commit. A session that
starts during the download causes `Error::Busy` at commit. A session attempting
admission during commit receives `Error::Busy`. Every participating process must
honor this protocol; a boolean idle snapshot is insufficient. Before shutdown or
pending-helper launch, the caller must stop its resident loop from admitting new
sessions. No silent elevation or permission changes are attempted.

The executable and its containing directory must be trusted local paths. The
existing executable must be a regular non-symlink file; setuid/setgid executables
are rejected. Installation requires write access for sibling staging/backup files;
a system package or read-only macOS bundle may require an operator-managed update.
Checking and ordinary session admission still work for read-only installations,
because their locks are in user state storage.

## Release and artifact verification

Only strict SemVer with higher precedence than the running version is eligible.
An optional single `v` prefix is accepted on release tags, not on the constructor's
running version. Drafts, GitHub prereleases, SemVer prereleases, equal versions,
build-metadata-only changes, and downgrades are excluded. Missing latest release
returns `None`; malformed metadata, missing/duplicate required assets, and missing
SHA-256 digests are errors. The GitHub `/releases/latest` stable release is
authoritative; neither source `main` nor a self-hosted manifest is consulted.

Artifact names follow `scripts/release.py`, exactly:
`<component>-<tag>-<target>.tar.gz`, or `.zip` on Windows.

| Targets | Executable validation |
| --- | --- |
| Windows `x86_64-pc-windows-msvc` | PE32+, AMD64, executable and not DLL |
| macOS `x86_64-apple-darwin`, `aarch64-apple-darwin` | Thin Mach-O 64-bit, correct CPU and executable type |
| Linux `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu` | ELF64 little endian, correct machine, executable/PIE type |

Musl and other unpublished targets are rejected. Header checks do not prove OS
library compatibility, platform signing, publisher identity, or a working runtime.
The updater never runs `--version` on the candidate. The selected tag and checked
archive establish the distributed version within the GitHub TLS trust boundary.

The client uses reqwest/rustls with HTTPS only, a 10-second connect timeout,
20-second metadata request timeout, 180-second per-download timeout, and at most
three followed redirects. It accepts only the fixed repository API/download
paths and GitHub's `release-assets.githubusercontent.com` release-asset host.
Credentials, nonstandard ports, fragments, HTTP downgrades and other hosts are
rejected. There are no automatic retry/replay loops.

Metadata is limited to 2 MiB, SHA256SUMS to 128 KiB, and each archive/downloaded
binary to 256 MiB. Downloads are streamed and counted, including when Content-Length
is absent. Advertised size must match actual size. SHA256SUMS itself must match its
GitHub SHA-256 digest; the exact archive entry must exist once in SHA256SUMS; the
archive must match **both** digests. Missing, duplicate or malformed checksums fail
closed. SHA-256 uses the `sha2` crate, not a custom crypto implementation.

Extraction is selective: only the exact expected product binary in its exact
archive root is written. Every entry is inspected, including ignored documents.
Absolute paths, `..`, `.`, backslashes, drive prefixes, control characters, unexpected
roots, duplicate names, links, hard links, devices and other special entries are
rejected. Only a bounded timestamp-only local PAX header is accepted for compatibility
with Python tarfile packaging. PAX path/link overrides and other extensions are rejected. ZIP encryption, ZIP64, multidisk, prefixed executables and ambiguous ZIP
layouts are rejected. At most 1,024 entries and 512 MiB of expanded data are allowed;
ignored data and tar padding count toward bounds. ZIP CRC and gzip trailer errors
fail verification. No `extract`/`unpack` operation writes an entire archive tree.

These checks provide TLS-based distribution trust and corruption detection.
Current official binaries are **unsigned**: there is no publisher digital signature,
Authenticode claim, Developer ID signature or notarization claim.

## Commit, deferred Windows replacement, and recovery

A verified candidate is staged alongside the destination, synced, and given ordinary
Unix executable permissions. Under the exclusive gate, the old executable is copied
to a unique sibling backup and synced. Its digest is checked again before commit.
Unix uses a same-directory atomic rename over the destination. It never moves or
removes the old executable before rename, so a failed rename preserves the old path.
The old running process continues on its original inode. The backup is retained on
success and may also remain after a failed final rename. Directory sync is attempted
following commit; power-loss durability still depends on the filesystem/storage.

Windows preparation retains a verified sibling candidate, backup and UTF-8 BOM
PowerShell script. `pending.launch()` rechecks the script SHA-256 and starts the
system `powershell.exe` using separate `Command` arguments, with no shell string,
no execution-policy bypass, no elevation and no automatic restart. Configured
execution policy may still reject an unsigned helper; `launch` waits at most 15 seconds
for a PID-specific readiness acknowledgement and reports `HelperFailed` on rejection
or startup timeout. Readiness is not completed installation. `pending.status` records
`ready:<PID>`, then `installed` or a failure diagnostic for later inspection. The script remains available for inspection
and explicit operator invocation under that machine's policy.

The helper waits up to 180 seconds for the original PID to exit, then waits up to
180 seconds for the shared session lock to become exclusive. It uses .NET byte-range
locking against the same fs2 lock range. It rechecks file types, stage/old/backup
SHA-256 and uses .NET `File.Replace`, preserving the old executable as backup.
If replacement reports an error after the destination vanished, it restores from
the validated backup through a sibling file. A changed existing destination is never
overwritten by recovery. Errors retain available candidate/script/backup files for
inspection. Run an installed binary only after the application has explicitly entered
restart state. Do not claim native Windows replacement was tested based on Linux
staging tests or cross-compilation alone.

For manual recovery, stop every process/session using this installation, inspect the
backup, and use an operator-controlled same-directory replacement. Do not blindly
start a backup or automatically replay failed updates. No updater removes backups
or stale pending scripts automatically; they can be inspected and deleted after a
successful restart. Preserve the permanent session lock file.

## Opt-in real GitHub verification

The example is separate from unit tests and uses no fake server or override endpoint:

```bash
cargo run --locked -p lightremote-updater --example official_release -- --expect v0.1.1
cargo run --locked -p lightremote-updater --example official_release -- --install --expect v0.1.1
```

The first command checks from `0.0.0`, printing the selected official release and
headless asset. `--expect` asserts the exact tag, so a future release causes an
explicit failure instead of silently changing the evidence. Omit it to follow a
future official stable release.

`--install` creates a fresh private temporary directory (normally under `/tmp` on
Linux), writes an old dummy there, and installs only into that dummy's path. It
verifies that the backup contains the old dummy, retains the directory, and prints
its path. No supplied destination is accepted, no current executable is replaced,
and no candidate is executed. On Windows it verifies safe pending preparation and
retains the script without launching it. Installation success proves that the live
archive passed GitHub digest, SHA256SUMS, layout and machine checks. Live GitHub is
never required by unit tests.

## Dependencies and validation

The crate uses reqwest/rustls for HTTP/TLS, semver for version precedence, sha2 for
SHA-256, flate2/tar/zip for archives, serde/serde_json for metadata, tempfile for
private unique staging, fs2 for cross-platform shared/exclusive locks, Tokio for
async I/O/blocking work, thiserror for typed public errors, and libc on Unix only
for `O_NOFOLLOW`/`O_CLOEXEC` and effective user identity. These are registry dependencies;
no reference-project source was copied. Dependency/license integration into the
repository's central THIRD_PARTY.md is owned by the parent integration task.

Focused validation after workspace/lock integration:

```bash
cargo test --locked -p lightremote-updater
cargo clippy --locked -p lightremote-updater --all-targets --all-features -- -D warnings
cargo doc --locked -p lightremote-updater --no-deps
```

Repository-wide checks and actual Windows/macOS execution are separate integration
and hosted-OS evidence. Web builds should use a same-origin deployment manifest
for refresh decisions; an official GitHub release link is only an install/deploy hint,
not evidence that the currently deployed browser app has changed.
