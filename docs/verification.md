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
- NSH Linux native controller: build, check, Clippy and 16 tests passed, including
  encrypted clipboard request/error handling added after the scoped review. JPEG
  dimensions are admitted before allocation on each supported platform.
- Python packaging fixtures verified ZIP/TAR contents, documentation/license
  inclusion, and executable modes. Workflow actionlint/ShellCheck passed.

Independent architecture and implementation reviews are in `review.md` and
`implementation-review.md`, including resolved findings and scope limits.

## Hosted build and release evidence

Source checkpoint `3d0aa0b`, hosted CI run `42285530612`: **completed, success**
(all ten jobs).

- Rust headless/default format, lint, tests and CLI PTY/files acceptance passed.
- Web typecheck, 34 unit tests, production bundle and five Chromium tests passed.
- Cloudflare typecheck and 10 workerd tests passed.
- Docker image build, container health and static Web client smoke passed.
- Actual Chromium → Rust relay → encrypted Rust host passed PTY output, resize,
  upload/download and truthful headless desktop rejection. The identical external
  relay contract passed against Rust and local Workers, including timer-driven
  idle expiry and one-time-ticket consumption.
- Linux x64/arm64, Windows x64 and macOS arm64 each built and packaged desktop
  CLI, headless CLI, native NSH controller and relay; macOS x64 also completed
  all four products successfully.
- Downloaded Linux x64/arm64 archives were inspected for actual ELF architecture,
  licenses/documentation and executable modes. The packaged Linux x64 headless
  and relay executables passed the same PTY and 2 MiB file acceptance script;
  `ldd` showed only libc, libm, libgcc and the loader, without GUI libraries.

The tag workflow rechecks the tagged source, assembles the complete per-platform
archive set and generates SHA256SUMS before creating the GitHub Release. Its GHCR
job builds both Linux architectures; image visibility is distinct from repository
visibility and must be checked separately. See `distribution.md`.

## Runtime and deployment limits

No physical Windows/macOS/Linux desktop session acceptance has been recorded.
No publisher code signatures or macOS notarization are provided. Cloudflare
local tests and dry-run do not deploy to a live account. The initial JPEG desktop
profile is experimental; see `platforms.md` and `design.md` for limitations.
