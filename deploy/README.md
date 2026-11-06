# Self-hosted relay

Build the browser bundle first (`cd web && npm ci && npm run build`), then build from the repository root:

```sh
docker build -f deploy/Dockerfile -t lightremote-relay:local .
export LIGHTREMOTE_RELAY_TOKEN="$(openssl rand -hex 32)"
docker compose -f deploy/compose.yaml up -d --build
```

Compose binds to `127.0.0.1:8787`; put an HTTPS/WebSocket-capable reverse proxy in front. Pass the public HTTPS origin in `LIGHTREMOTE_PUBLIC_ORIGIN`, and configure `LIGHTREMOTE_ALLOWED_ORIGINS` for independently hosted browser clients. Preserve the WebSocket protocol request header; avoid credential/header/query logs. Room credentials are sent only via `Sec-WebSocket-Protocol`, and the server negotiates only `lightremote.v1`.

The image runs as unprivileged UID 10001. Compose uses a read-only filesystem, drops capabilities, sets process/memory bounds and requires a supplied operator token. The executable exits if the token is absent. Rooms are memory-only and disappear on restart. Maximum configured room buffering can exceed the container memory setting; choose `LIGHTREMOTE_MAX_ROOMS` to fit the deployment. See `../crates/relay/README.md` for bounds and origin defaults.

The image includes the prebuilt `web/dist` bundle at `/web` and sets `LIGHTREMOTE_WEB_DIR=/web`. To replace it without rebuilding the image, mount a read-only volume `../web/dist:/web:ro`. Same-origin hosting is convenient but lets the relay operator replace browser code and steal encryption secrets. An independently trusted client origin is preferable for stronger end-to-end trust; configure an explicit allowlist for it.

The Dockerfile builds a standalone copy of the relay package so unrelated desktop/workspace dependencies are excluded. It uses the standalone `crates/relay/Cargo.lock` and builds with `--locked`; the root workspace lock is not required in the build container. Release automation supplies the built web artifact before building the image. Pin image digests for reproducible production artifacts. No Docker daemon or Cloudflare account is required for Rust/Workers conformance tests.

`./deploy/smoke-relay.sh ./target/debug/lightremote-relay` starts an existing binary with env configuration and checks real HTTP health/auth/room creation. It uses `LIGHTREMOTE_BIND` or `127.0.0.1:18787` by default. The script never prints returned room tickets and kills its child on exit. Full WebSocket conformance lives in `crates/relay/tests/conformance.rs` and `cloudflare/test/relay.test.ts`.

Run **the same external contract suite** against either server (Python 3 standard library only):

```sh
LIGHTREMOTE_RELAY_TOKEN="$LIGHTREMOTE_RELAY_TOKEN" \
  python3 deploy/relay-conformance.py http://127.0.0.1:8787
# Or point exactly that script at Wrangler dev / your own deployed relay URL.
```

Use an isolated test deployment: the suite allocates and consumes several rooms. It checks real RFC 6455 handshakes and masked frames, operator/role authentication, duplicate roles, pre-peer hello delivery, binary forwarding including fragmented records, both raw and fragmented oversized messages, text rejection, queue overflow and teardown. It does not print room tickets. The test assumes the default same-origin policy and room limits sufficient for its small sequential run; it leaves no live rooms on success.

`./deploy/test-relay-pair.sh ./target/debug/lightremote-relay` starts both local implementations and executes the shared suite against each. It requires `npm ci` in `cloudflare`, Python 3 and curl; it uses ephemeral local ports and isolated temporary Wrangler config, secret and state files.

The pair runner configures a four-second absolute room lifetime and a one-second waiting deadline in its isolated servers. It passes `--wait-seconds 1` to the shared suite, which waits for actual timer-driven socket closure with no intervening HTTP request. Both an unpaired socket and a connected pair must close, and their room tickets must then return 404. The standalone suite leaves these real-time checks disabled unless `--wait-seconds` is supplied; use that option only with matching short-lived server configuration (room TTL at most ten seconds). The returned expiry timestamp check alone does not prove timer behavior.

Regular push/PR CI should also build `deploy/Dockerfile` after downloading the `web-dist` artifact, with image publishing disabled. The tag release image build alone detects container packaging failures late. The Dockerfile requires `web/dist` and the owned standalone relay lockfile; its build context is the repository root.

## Paired-backpressure verification snapshot

Recorded at UTC: `2026-11-06T19:38:45+00:00`. This snapshot refers to the working-tree source hashes below. No core files were changed by the relay fix.

The original **7** Rust relay integration tests remain; **3** paired-pressure regressions were added, making **10** total. The failure was reproduced both in the original `tiny_windows` core test and in a relay-only simultaneous burst test. Temporary diagnostics confirmed `destination queue FULL; peer joined=true` immediately before room consumption and both carrier-close errors. Those diagnostics were removed from the final source.

Paired sockets now retain at most one additional pending inbound record, reserve bounded peer-queue capacity, and continue draining their own queue while waiting. Fair selection prevents fixed-priority starvation. Pending forwarding and socket writes have five-second deadlines, including cancellation. Before a peer joins, the explicit eight-record overflow policy is preserved. The three new regression cases cover simultaneous 512 KiB-per-direction bursts with TCP_NODELAY on current-thread and two-worker runtimes, plus a paired destination that stops reading and must hit its deadline rather than buffer indefinitely.

| Exact command | Verified result after the relay fix |
| --- | --- |
| `cargo test -p lightremote-relay --locked -- --nocapture` | 10 integration tests passed, including all 7 original cases and all 3 pressure regressions |
| `cargo clippy -p lightremote-relay --locked --all-targets -- -D warnings` | Passed with warnings denied |
| `rustfmt --edition 2024 --check crates/relay/src/*.rs crates/relay/tests/*.rs` | Passed |
| `cargo test -p lightremote-core tiny_windows --locked -- --nocapture` | 10 consecutive runs passed on the final fair-selection implementation |
| `cargo test -p lightremote-core --locked -- --nocapture` | Latest main integration check passed: 9 unit tests and 1 integration test, including tiny-window/GOAWAY |
| `cargo doc -p lightremote-relay --locked --no-deps` | Passed |
| `cargo build -p lightremote-relay --locked` | Passed; produced the executable used by the pair runner |
| `(cd cloudflare && npm test)` | 10 Workers-runtime tests passed; refreshed while recording this snapshot |
| `./deploy/test-relay-pair.sh ./target/debug/lightremote-relay` | Passed against both the fixed Rust executable and local Wrangler runtime, including actual waiting/absolute-expiry socket teardown |

The shared external contract did not need to change for paired backpressure: API, record ordering, size limits, teardown, and waiting-phase overflow behavior remain the same. Its existing binary/fragmentation/oversize/text/queue-overflow/auth/duplicate-role tests passed against both implementations after the fix. Rust-specific pressure branches are tested in the new relay integration cases. Cloudflare source and its ten-test suite were not changed by this fix. Broad suites were not repeated solely to write this documentation; the table records the completed runs, with the small CF10 suite refreshed to confirm its current result.

SHA-256 values for the verified source and harness files:

```text
331891734411c1d943fc80c5718638b456d8050d13d377fd3341182c0dd3b4bd  crates/relay/src/lib.rs
6b584de14cd64f158d1be03455296101af0298212cf21a83bc49686185e4ab83  crates/relay/tests/conformance.rs
a6b2422454112a067f78a4bfbf8e51d91187a32b43f5c1a1c56428c9932a589b  cloudflare/src/index.ts
23a878f71a1a0317c51502086d3fab2adf686388ba6b3ca0a20087632bf7f01c  cloudflare/test/relay.test.ts
e7e51e489a9266c5118785dc209c9c1f34923d536da46dee26eb044b2720eef3  deploy/relay-conformance.py
d40430d6a35ed2833a177819ab52e3534afe592b44ef195fc4e42d59aeb0f3c8  deploy/test-relay-pair.sh
```

These are source/harness hashes, not Docker image or deployed-Worker hashes. Docker execution remains a CI validation gate because no local Docker daemon is available; account-level Cloudflare deployment/crash injection is outside this local runtime evidence.
