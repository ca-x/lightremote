# LightRemote capability map

| Module | Responsibility | Depends on |
| --- | --- | --- |
| app-contract | Versioned profiles, bounded application records, capabilities and explicit errors | TMP v1 |
| secure-link | Authenticated encrypted records over byte carriers, single-owner TMP driver | app-contract |
| relay | Room tickets, bounded opaque WebSocket forwarding; Rust/Docker and Cloudflare DO | secure-link carrier contract |
| host-services | Desktop/input, PTY, sandboxed files, clipboard, screenshots/events | app-contract, secure-link |
| permissions | Host-owned capability ceiling, attended requests and revocation generations | host-services |
| updater | Official GitHub stable discovery, digest verification, recovery and session exclusion | GitHub release API |
| daemon | Independent headless resident host and OS service lifecycle | host-services, permissions, updater |
| web-client | Browser connection, desktop, terminal, files, clipboard | app-contract, secure-link |
| native-client | NSH native UI backed by Rust networking and services | app-contract, secure-link |
| distribution | Versioned builds, public repository, platform artifacts and relay container | application modules |

Build order: app-contract → secure-link, relay → host-services, web-client → native-client → distribution and end-to-end acceptance.

The owner delegated architecture evaluation to independent agents. Review results and resulting changes are recorded in docs/review.md; no additional human design gate is introduced.

## Published v0.1.1 platform evidence (2026-11-07)

`Pending` means no successful evidence recorded yet. CI build success and physical
runtime verification are different checks. Initial desktop/native artifacts are
experimental until a real OS session is tested.

| Target | Backend / capability | Local build | Automated functionality | Physical desktop | Signing |
| --- | --- | --- | --- | --- | --- |
| Linux x64 headless | portable-pty; cap-std files | Build passed | 45 tests + PTY/files encrypted E2E passed | N/A | Unsigned |
| Linux x64 host desktop | xcap; enigo X11; arboard | Build/check passed | Desktop unit/service tests passed; live test skipped | Not verified | Unsigned |
| Linux Wayland | Partial upstream capture; no complete native injection path | Experimental | Not verified | Not verified | Unsigned |
| Linux x64 NSH | Native Rust UI / GTK4 + native input surfaces | Build passed | 16 tests passed; explicit clipboard controls | Not verified | Unsigned |
| Linux arm64 | Host, relay, headless, NSH / CI runner | Hosted build passed | Archive/ELF checks passed; OS runtime unverified | Not verified | Unsigned |
| Windows x64 | xcap, enigo, portable-pty / NSH | Hosted build passed | OS runtime unverified | Not verified; UAC/session0 unsupported | Unsigned |
| macOS arm64 | xcap, enigo, PTY / NSH AppKit | Hosted build passed | OS runtime unverified | Not verified; permissions required | Unsigned, not notarized |
| macOS x64 | xcap, enigo, PTY / NSH AppKit | Hosted build passed | OS runtime unverified | Not verified; permissions required | Unsigned, not notarized |
| Web | WebCrypto, released TMP JS core, canvas, xterm | Build passed | 37 unit + 5 Chromium + real Rust PTY/files E2E passed | Physical capture not verified | N/A |
| Rust relay | axum WebSocket, bounded rooms | Build passed | 10 relay tests + encrypted half-close/tiny-window tests passed | N/A | Unsigned |
| CF relay | Worker + Durable Object | Typecheck/bundle passed | 10 Workers tests + shared external conformance passed | No live account deployment | N/A |

See docs/review.md for the scoped review and docs/verification.md for
commands and CI run identifiers. No physical desktop support is inferred from a
protocol fixture or compilation alone.
