# Permissions, resident operation and update verification

Baseline: published v0.1.1 (`4e33afa`). The changes on
`feat/permissions-resilience-updates` prepare the next version; they do not alter
the published release, its assets or immutable tags. TMP remains `tunnel-mux`
0.1.0; permissions are LightRemote application semantics.

## Review scope and corrections

Independent agents reviewed host permissions/lifecycle, carrier/updater behavior,
and daemon deployment. Integration review covers manifests, client contracts and
artifact mapping. The implementation includes these corrections identified in review:

| Finding | Resolution |
| --- | --- |
| A queued blocking file operation could survive revocation and regrant | Abort queued work and recheck the original permission generation before OS access; track started jobs through shutdown. Already-started syscalls cannot be rolled back. |
| Deferred PTY reaping could outlive the session admission lock | A session cleanup token follows the child into the reaper; host shutdown waits for actual reaping. |
| Two resident hosts could race to publish or clear an invitation | Retain an exclusive permanent writer lock through cleanup; reject a second publisher and unsafe invitation paths. |
| A fork-inherited descriptor could briefly retain the writer lock after host shutdown | Explicitly unlock the unique writer authority on drop; the stable lock file remains in place. A duplicated-descriptor regression failed before this fix. |
| Fixed RPC budgets truncated slow-profile OPEN attempts | Carry the chosen network profile through CLI control and permission exchanges. |
| Browser send buffering escaped the write deadline and started PONG timing too early | Keep the original deadline through browser buffer drain; start PONG/RTT timing only after drain, preserving early matching replies. |
| Service startup depended on a login HOME despite configured state | Use the daemon's permanent `state_dir/sessions.lock` for both sessions and updates, including deferred helpers. |
| Windows SYSTEM installer trusted an arbitrary binary path and installer identity | Validate executable, ancestors and private config under the target SYSTEM/Administrators policy before executing or registering the binary. |
| The installer rejected the standard ProgramData ancestor ACL | Permit ancestor metadata and unrelated child creation while still rejecting deletion, replacement and ACL/owner changes; keep executable, immediate parent and private leaf rules strict. |
| A custom desktop daemon could auto-update into the official headless product | Reject desktop capability policy combined with automatic daemon updates until a matching official product exists. |
| Client control requests could remain queued across revocation and regrant | Pending scoped calls must retain their original authorization lifetime and be cancelled before transmission when it ends. |
| Native grants could enable an unavailable OS backend | Effective availability must also honor the host's capability flags and routes; permission grants alone are insufficient. |

## Local evidence

These checks use real implementations unless explicitly described as fixtures.
Physical capture/input and system service installation are separate checks.

| Command or check | Evidence |
| --- | --- |
| `bash scripts/check.sh` | Rust default/headless formatting, strict Clippy and tests; Web typecheck, 73 unit tests and production build; Workers typecheck and 10 tests. Follow-up Rust checks cover subsequent lifecycle fixes. |
| `cargo test -p lightremote-native` and strict native Clippy | 40 tests, including persistent update-check opt-out, effective backend availability and permission revocation. |
| `npm run test:browser` | 11 production Chromium tests, including approval/denial/cancel, revoke, transfer cancellation, update consent and heartbeat degradation. |
| `npm run test:rust` with real CLI/relay binaries | Real Chromium, encrypted Rust host, PTY output/resize, 700 KB download, 500 KB upload, and disabled headless desktop actions. |
| `scripts/e2e-cli.py` | Real PTY, 2 MiB atomic upload with hash receipt, download, failed download cleanup. |
| `scripts/e2e-host.py` | Real TTY deny/approve/revoke, fresh consecutive invitations, relay outage recovery and SIGTERM cleanup without secret logs. |
| `crates/daemon/tests/e2e.py` | Independent daemon, real relay and two fresh encrypted sessions; private invitations, permissions default off, shutdown and redacted logs. |
| Daemon lifecycle process tests | No HOME/XDG requirement; shared update/session lock is exclusive against a running host; `--check-config` does not contact the relay or create state. The no-HOME regression failed before the gate fix and passed after it. |
| Official updater example with `--install --expect v0.1.1` | Downloaded the actual official headless asset, verified GitHub digest and SHA256SUMS, installed into a fresh temporary dummy path and retained its original contents in the backup. No installed executable was replaced or run. |
| `cargo doc --no-deps` for core/services/CLI/updater | Rustdoc generation passed. |
| Daemon dependency tree | Default headless build excludes capture, input, clipboard and NSH dependencies. |
| Release packaging with temporary fixture binaries | Five products × five targets plus Web; 28 files including notice/checksums; all hashes, Unix daemon executable mode and bundled daemon deployment instructions checked. This validates packaging, not platform executability. |
| Service templates and workflows | JSON/plist parse and host shellcheck passed; actionlint passed with only the existing `macos-15-intel` label excluded because actionlint 1.7.7 predates that hosted label. |

## Hosted and runtime evidence

Hosted platform CI is available from the [feature branch workflow runs](https://github.com/lib-x/lightremote/actions/workflows/ci.yml?query=branch%3Afeat%2Fpermissions-resilience-updates). Its result remains separate from the local evidence above. The matrix must build
Windows x64, macOS x64/ARM64 and Linux x64/ARM64 separately, including the new
headless daemon product. A successful build does not prove an installed service
or physical desktop works.

The native Windows installer suite executed 48 assertions successfully on a hosted
Windows 2022 runner, including protected temporary paths, writable binaries and
parents, private state, personal owner rejection and junctions. This exercised the
administrator-token branch rather than skipping it; it made no SCM calls. The
independent fast CI job now catches these trust checks before platform compilation.

Not verified here: physical desktop capture/input, Windows SCM installation and
stop/recovery, real Windows deferred executable replacement, macOS launchd/TCC,
publisher signatures/notarization, and deployment to a live Cloudflare account.
No system service was installed on the development machine. Official binaries
remain unsigned. The existing invitation identity and WebSocket relay architecture
remain in use; persistent device pairing, PFS, P2P, SSH and QUIC integration require
separate work.
