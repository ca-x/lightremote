# roc-remote web controller

Browser controller for the v0.1 contract in `../docs/design.md`. Built with TypeScript, Vite, and xterm. All production dependencies are browser code; there are no Node polyfills or sibling workspace dependencies.

```sh
cd web
npm ci
npm run verify:sdk
npm test
npm run build
npm run test:browser
```

`dist/` is the static artifact for the relay or CLI to serve. `npm run dev` starts a local development server. Browser tests build and serve the production bundle on `127.0.0.1:4173`. They use `/opt/google/chrome/chrome` or `/usr/bin/chromium` if present, otherwise Playwright Chromium. Set `CHROMIUM_PATH` to override, or run `npx playwright install chromium`.

## SDK provenance and browser boundary

`@lib-x/tmp` is installed from the immutable language-release asset specified in `package.json`, with integrity recorded in `package-lock.json`. The GitHub release API asset digest and downloaded archive were compared:

- Release: `lib-x/tm`, tag `sdk/node/v0.1.0`.
- Asset: `tunnel-mux-node-0.1.0.tgz`, 37,766 bytes.
- SHA-256: `ff3d5931781beeeb050de88f9fe3033f4810d6a6d122e399505bbca8f7be3cf9`.
- `npm run verify:sdk` repeats the size/digest verification without writing an archive.

The SDK root imports its Node transport, including `node:timers/promises`. `src/link.ts` imports **only the installed package's `dist/session.js` and `dist/framing.js` files by relative path**. This is deliberate: both Vite and Node tests resolve the pinned implementation without importing the root, using a fork, duplicating the state machine, or needing a Node-only alias loader. Types come from the installed declarations. The production Chromium smoke exercises real encrypted SDK streams, so browser compatibility is verified beyond tree-shaking assumptions.

## Connection and security

An invitation is a URL fragment containing unpadded base64url UTF-8 JSON with exactly `{relay,room_id,client_token,secret}`. `secret` decodes to 32 bytes. A hash invitation pre-fills the form and is removed from browser history immediately; the user presses Connect. Pasted links and bare fragments also work. Secrets are never stored in local/session storage or logged. A used room requires a fresh invitation to reconnect.

Tickets are sent only as the WebSocket subprotocol `client.<ticket>`, alongside `lightremote.v1`. The relay must select `lightremote.v1`. Remote relay URLs require TLS; HTTP/WS is permitted only on loopback. Base paths on the relay URL are preserved.

`src/secure.ts` implements the design's draft v0.1 PSK bearer construction unchanged: binary JSON hello, independent fresh 32-byte nonces, HKDF-SHA256, directional AES-256-GCM, bound room/role AAD, exact u64 counters, and a 150,000-byte carrier limit. Possession of the invitation grants scoped session access. This is not an independent host identity or PAKE protocol. Each directional key is capped at 2^32 records and 64 GiB of cumulative plaintext, matching the Rust core; an operation exceeding either budget destroys the key and closes the link. The counter stays u64 on the wire. Session keys are discarded on close and connections have a 20-second encrypted TMP negotiation timeout.

Serve this static client from an independently trusted HTTPS origin when the relay operator is not trusted to serve client code. The relay's CORS/Origin configuration must allow that origin. The page uses local assets, no external font or analytics requests, a no-referrer policy, and a CSP. A serving HTTP header `frame-ancestors 'none'` is recommended because CSP meta tags cannot enforce it.

## Service behavior and bounds

- One TMP owner, FIFO protocol replies before event-induced packets, independent carrier read/write progress, and retries on every received packet, including WINDOW.
- GOAWAY rejects new opens while accepted streams and WINDOW/output processing continue. Draining has a 15-second deadline. After all application chunks, including empty FINs, are consumed, an end-to-end Ping/Pong barrier confirms delivery of final queued packets before carrier close. Async processing rechecks stream failures before returning credit and before FIN success; failed streaming downloads abort their sink.
- 16 local concurrent streams; 128 KiB stream receive and send queues; 1 MiB connection receive window; 2 MiB carrier queues plus bounded queue counts. RESET cancels pending operations. FIN half-closes only the sending direction. `MuxStream.read()` returns an explicit `consume()` acknowledgement; `process()` consumes each chunk after its async handler completes.
- Application framing tolerates arbitrary splits/coalescing and keeps one partial record, at most 8 MiB. Control is capped at 1 MiB with at most 32 pending requests. Open/request timeouts are 15 seconds.
- Desktop admits JPEG marker dimensions before pixel decoding: at most 16,384 pixels per axis and 16,777,216 pixels total. The bounded marker parser rejects malformed/truncated segments, later frame headers, and height overrides; both live frames and screenshot exports pass it. Canvas decoding processes one record at a time and checks the decoded dimensions against the admitted header. Single-frame screenshot export uses a separate desktop stream. Missing capture support or permission errors remain visible. Production has no fixture frames or simulated success paths.
- Pointer coordinates scale to intrinsic image dimensions. Input requires an explicit checkbox. Input is bounded and never replayed. Blurring/hiding the window, stopping the view, or closing the session resets input; the host releases held input. Canvas focus loss sends key/button releases. `Send text` supports composed/non-Latin text.
- Terminal uses raw PTY bytes and xterm, with resize control requests and a bounded 5000-line scrollback. Close resets the stream and lets the host terminate its child process.
- File paths are relative to the host share root. Browse, mkdir, nonrecursive remove, upload, and download are supported. Upload sends an exact byte count, FIN, then validates the host's `{size,sha256}` receipt. It does not claim independent local checksum verification. Download FIN preserves the reply direction. Where supported, downloads write directly to a user-selected file; the in-memory fallback is capped at 64 MiB. Only one UI file transfer runs at once.
- Clipboard reads and writes are explicit buttons. Connecting never reads either clipboard.

Host response shapes are aligned with `crates/services`: capabilities are an object of permission booleans plus `routes`/`limits`; display enumeration is `{displays:[{id,name,width,height,...}]}`; listing is `{entries:[{name,type,size}]}`; clipboard get is `{text}`. These shapes fill omissions in the draft design, without changing its wire contract.

## Verification scope

Node tests independently compare WebCrypto against Node HKDF/AES-GCM, reject tampering/replay/skips/wrong keys/room/direction, validate invitations and framing, and use the actual SDK to exercise explicit credit, FIN, upload, stalled writes, and RESET. `secure.ts` and `link.ts` can be imported by Node with `tsx`; no browser DOM is evaluated at import time.

Playwright runs the production bundle in real Chromium at desktop, 375 px and 320 px. The encrypted host fixture deliberately splits/coalesces TMP packets and uses tiny credit windows. It covers terminal input/resize, large file transfers, explicit clipboard, headless capture failure, JPEG rendering, screenshots, and pointer/key/text records. That fixture is **test-only**: its echo and JPEG validate browser wiring, not a physical desktop backend or a real OS PTY. The optional real Rust acceptance below verifies the PTY and files independently. Native permissions and physical capture remain separate acceptance checks.

## Real Rust acceptance

With prebuilt relay and host binaries, run:

```sh
npm run build
ROC_REMOTE_BIN=/absolute/path/to/roc-remote \
ROC_REMOTE_RELAY_BIN=/absolute/path/to/roc-remote-relay \
npm run test:rust
```

This starts a loopback Rust relay serving `dist`, a headless Rust host with a temporary share under `web/test-results`, and Chromium. It verifies real `/bin/sh` PTY output, host-applied `stty size` after resize, a 700 KB download, a 500 KB upload checked against the host filesystem, and an explicit unavailable-desktop error. It never builds or changes sibling sources and does not print the invitation. The run passed against the locally prebuilt binaries on 2026-11-07. Physical capture is not claimed.

Tiny credited DATA packets coalesce only while queued and undispatched. The
128 KiB receive window bounds their combined bytes; geometric storage growth
avoids quadratic copying, and an already-delivered chunk is never mutated.
A genuine FIN remains after its queued payload, with credit returned once upon
application consumption. Packet count alone does not reset a valid input burst.
