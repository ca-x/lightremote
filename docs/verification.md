# Verification record

Evidence collected on 2026-11-07. Compilation, automated functionality, physical
OS sessions, signing, and external deployment are independent claims.

## Local Linux verification

- `bash scripts/check.sh`: exit 0; headless Rust 43 passed, desktop Rust 45
  passed and one live-capture test ignored; web typecheck, 34 tests and production
  build passed; Cloudflare typecheck and 10 workerd tests passed.
  Local log: `/tmp/lightremote-final-checks-3.log` (not a published artifact).
- `python3 scripts/e2e-cli.py`: real encrypted relay/host/controller session;
  capabilities, 2 MiB upload and download, SHA-256 receipt, missing-download
  cleanup, and actual PTY command execution passed.
- Production Chromium: five fixture tests plus real Chromium → Rust relay → Rust
  host PTY/file acceptance passed. See `web/README.md` for commands.
- Shared relay contract passed against both Rust and local Workers runtime;
  tiny-window encrypted roundtrip passed ten consecutive runs. Exact relay
  commands and source checkpoint hashes are in `deploy/README.md`.
- NSH Linux native controller: build, check, Clippy and 14 tests passed. JPEG
  dimensions are admitted before allocation on each supported platform.
- Python packaging fixtures verified ZIP/TAR contents, documentation/license
  inclusion, and executable modes. Workflow actionlint/ShellCheck passed.

Independent architecture and implementation reviews are in `review.md` and
`implementation-review.md`, including resolved findings and scope limits.

## Hosted build and release evidence

Pending the initial public source push. CI run identifiers and release assets
will be recorded after the hosted jobs complete. Docker image build/health/static
client smoke, Windows/macOS native builds, and Linux arm64 builds have not yet
been established by local verification.

## Runtime and deployment limits

No physical Windows/macOS/Linux desktop session acceptance has been recorded.
No publisher code signatures or macOS notarization are provided. Cloudflare
local tests and dry-run do not deploy to a live account. The initial JPEG desktop
profile is experimental; see `platforms.md` and `design.md` for limitations.
