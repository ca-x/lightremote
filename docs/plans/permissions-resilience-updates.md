# LightRemote permissions, resilience and updates

Baseline: v0.1.1 / 4e33afa. User authorized continued implementation and autonomous agent review, then explicitly requested rust-best-practices. Work proceeds on feat/permissions-resilience-updates in an isolated worktree.

## Capability map and order

| Module | Contract | Depends on |
| --- | --- | --- |
| permissions | Host-owned grants, requests, denial and revocation | Existing services |
| resilience | Bounded connection budgets, health, RTT and black-hole failure | Existing TMP carrier |
| updater | Trusted release discovery, verification, staged replacement and recovery | GitHub releases |
| clients | Permission UI, health and idle-only update controls | Three modules above |

Implement the host permission boundary locally; delegate carrier, updater, Web and native work with disjoint write scopes. Review integration independently after implementation.

## Permission contract

Scopes: desktop, input, shell, file_read, file_write, clipboard_read, clipboard_write. Configuration is the immutable ceiling. Attended sessions start with no grants; only the local host may approve or revoke. An unattended operator explicitly grants the configured ceiling. Remote requests cannot grant permissions.

`permissions.get`, `permissions.request {scopes}`, and `permissions.cancel {id}` return `{available,granted,pending:{id,scopes}|null,revision,mode,last_decision:{id,outcome}|null}`. One bounded pending request exists per single-controller session; excessive/unknown/out-of-ceiling requests fail. Request IDs and revisions are monotonic and bounded. `permissions.changed` carries the snapshot. Denial removes the pending request, leaving grants unchanged. Revocation cancels affected active services; a grant generation prevents revoke/regrant from reviving previously accepted work. Independent stream permissions apply to control operations as well as OPEN. Clipboard read and write are separate grants.

CLI defaults to local attended approval, with `--unattended` for deliberately noninteractive hosts. The local prompt explains shell power, file read/write, clipboard direction and input. Only scope names and IDs are logged, never paths, keys, tickets, clipboard, terminal or file contents. Closing the local approval channel revokes grants. Every task is cancelled and joined during teardown.

## Resident operation

The host supports `--stay`: after a disconnect it releases all session workers and input, discards attended grants, and creates a new single-use invitation. Explicit unattended policy persists as the local operator configuration. SIGINT/SIGTERM cancel connection attempts, approval reads, service tasks and retry delays. Bounded exponential backoff prevents a busy loop when the relay is unreachable. Provide opt-in Linux systemd, macOS launchd and Windows background-task deployment templates; do not install them into the user machine during implementation. GitHub remains the authoritative update source.

## Weak networks

Keep all queues and record assembly bounded. Surface peer health and RTT, enforce handshake/open/write/heartbeat deadlines with testable budgets, and close black-hole connections. Distinguish slow service progress from a dead carrier. Do not replay input, shell commands, file commits, or consumed invitations. Reconnection requires a fresh invitation; this iteration does not add session resumption, file resume or P2P. Preserve FIN half-close, RESET failure and GOAWAY drain.

## Updates

Fixed lib-x/lightremote GitHub release source, stable SemVer upgrades only, exact product and target selection. Verify archive digest against GitHub metadata and SHA256SUMS; bound HTTP requests, redirects, downloads and extraction. Extract only a single correct binary, refusing links/traversal/duplicates. Install into a sibling staged file, preserving the old executable on failure and providing a recoverable backup. Implement an explicit Windows deferred replacement path without privilege escalation. Active sessions must not be replaced or silently restarted. Native clients may check periodically with an opt-out and expose available/install/restart states. CLI exposes explicit check/install commands. Web checks a same-origin version manifest and offers a safe refresh instead of disrupting a session.

TLS and GitHub artifact digests are the distribution trust boundary. Existing releases are unsigned; no publisher signature or OS signing claim is made. Do not publish new releases or change immutable existing tags as part of implementation verification.

## Acceptance and verification

- Permission defaults, ceiling validation, request/approve/deny/cancel and revoke/regrant are tested, including live shell/file cancellation.
- Web/native disable unsupported actions, display pending/denied/revoked permissions, and release input when permission or connection disappears.
- Deterministic tests exercise delayed/silent peers, backpressure, timer cleanup and deadline failure without long sleeps.
- Updater rejects downgrade, wrong product/target, bad digest and unsafe archives; failed install leaves the existing executable usable. Windows staging is verified separately from actual Windows process execution.
- Run formatting, focused Rust tests/Clippy/docs, Web tests/typecheck/build and relevant production-browser/CLI end-to-end tests. Full checks run at integration; physical OS tests, signatures and live Cloudflare deployment remain separate evidence.

## Rulings and progress

- Ruling: use local host attendance rather than a new remote identity service; the project remains one invitation/one controller, preserving the existing authentication boundary.
- Ruling: do not auto-reconnect consumed role tickets; safe resumption requires a separate protocol design and cannot be simulated by retries.
- Ruling: use the existing GitHub distribution trust boundary; adding a mandatory new signing secret would leave updating unusable for existing releases. OS signing is independent work.
- Implemented: permission authority and epoch cancellation; bounded carrier health and low-bandwidth presets; verified official GitHub updates; Web/NSH permission and update interaction; independent headless daemon with native SCM and systemd/launchd templates.
- Local verification: full Rust/Web/Workers checks, NSH tests and Clippy, real TTY/PTY/files/resident/daemon sessions, production Chromium to Rust, live official v0.1.1 updater probe, rustdoc and archive matrix checks.
- Independent review: host/lifecycle, updater/carrier, daemon/deployment, and client/manifests/packaging findings fixed and rechecked.
- Final integration: verify the feature branch with hosted platform CI. This branch creates no release or tag. See docs/hardening-review.md for evidence and remaining OS runtime limits.
