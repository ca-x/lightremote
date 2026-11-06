# LightRemote v0.1 architecture and acceptance

Status: implementation contract, independent evaluation in docs/review.md. Date: 2026-11-07.

## Objective and defaults

A remote desktop product with browser and NSH native controllers, a Rust agent with a separate headless feature set, and interchangeable self-hosted Docker / Cloudflare Workers + Durable Objects relays. Linux, Windows and macOS are release targets. Terminal, files, clipboard, screenshots and session events are application services on TMP; the TMP schema remains maintained by lib-x/tm. No reference repository is modified. Implementation will be original; reference provenance and dependency licenses are recorded separately.

Native desktop capture/input is an optional platform capability. Builds without desktop libraries must still provide authenticated file and shell services. Unsupported OS permissions or backends produce explicit capability errors. Verification distinguishes compile/test coverage, physical desktop tests and signed/notarized distribution.

## Relay API, identical on Rust and Cloudflare

- `GET /healthz` → `{ok:true,protocol:"lightremote-relay.v1"}`.
- `POST /v1/rooms`, Authorization Bearer operator token, body `{}` → `{room_id,host_token,client_token,expires_at}`. 256-bit random role tickets. Expiration ≤24h. Tokens are never placed in HTTP query strings.
- `GET /v1/rooms/:room/ws` upgrades with WebSocket subprotocols `["lightremote.v1", "host.<ticket>"]` or `["lightremote.v1", "client.<ticket>"]`. The server selects only `lightremote.v1`.
- Exactly one live host and controller per room. Duplicate role rejected, room tickets are checked before accepting traffic. Disconnect closes both ends and consumes the room; reconnection creates a fresh room/session. No silent endpoint replacement.
- Forward only binary records, max 150000 bytes; bounded per-room buffering, reject/close on overflow. An authenticated, bounded waiting phase permits host to connect before controller. Relay sees ciphertext and metadata only. Operator token, tickets and application data never appear in logs. CORS/Origin and resource quotas are explicit configuration.
- Cloudflare DO owns room state and WebSocket lifecycle. Relay persistence and room creation are bounded; no unauthenticated arbitrary room allocation. Deployed URLs/CF credentials are user-owned configuration, no live account mutation is required to build or test.

## End-to-end encrypted carrier

Invitation URL fragment carries relay base, room ID, controller ticket and a fresh random 32-byte encryption secret; fragments are never sent in HTTP requests. Agent retains its host ticket and secret. TLS authenticates relay transport in deployed setups; localhost ws is allowed for tests.

Per connection, peers exchange one bounded JSON handshake `{type:"lightremote.hello",version:1,role:"host"|"client",nonce:<base64url random32>}`. Both derive two AES-256-GCM directional keys with HKDF-SHA256, salt=client_nonce||host_nonce, info UTF8 `lightremote/v1/<room>/<sender-role>`. The first encrypted record confirms the negotiated transcript by authenticating a valid TMP Hello/Welcome. Input is not authorized before successful encrypted TMP negotiation. Counters start at zero independently; each direction is limited to2^32 records and64GiB plaintext per key, after which the connection fails closed and needs a fresh invitation; nonce=direction tag (4-byte big endian: client=1,host=2)||u64be counter; record=u64be counter||AES-GCM ciphertext+tag. AAD=UTF8 `lightremote/v1/<room>/<sender-role>`. Enforce exact next counter, no replay/skips, bounded record length, never reuse session key after close or restart. All roles, room and nonce values are bound through key derivation/AAD. Handshake timeout and connection close destroy session state.

Plaintext is `u32be length || Protobuf Packet` using tunnel-mux=0.1.0 / Node SDK language release. Carrier and application are separate; adding an SSH, QUIC or future adapter changes no TMP message. Media may later use a dedicated TMP link to avoid ordered-carrier head-of-line contention; v0.1 does not claim unreliable QUIC/RTP or seamless stream migration.

## Application contract

Negotiated profiles: `lightremote.control.v1`, `lightremote.desktop.v1`, `lightremote.input.v1`, `lightremote.shell.v1`, `lightremote.file.v1`. OPEN metadata is UTF-8 JSON, at most 4096 bytes. All application record streams use `u32be length || body`, with body bounded to 8MiB unless a smaller route-specific cap is specified; records may span arbitrary TMP DATA frames.

- `control`: bidirectional record JSON `{id,method,params}` → `{id,result}` or `{id,error:{code,message}}`. Notifications `{event,data}`. Methods: `capabilities`, `desktop.displays`, `shell.resize` `{stream_id,cols,rows}`, `fs.list` `{path}`, `fs.mkdir`, `fs.remove`, `clipboard.get`, `clipboard.set`. JSON control record max1MiB; errors do not expose arbitrary host paths or credentials.
- `desktop`: OPEN `{display:0,fps:10,quality:70,single:false}`. Host sends framed JPEG records. Image dimensions are intrinsic. `single:true` emits exactly one frame and FIN, supporting screenshot export. Frame limits, fps and display IDs are validated; no unbounded frame queue. Browser displays through a canvas/image renderer. Native controller receives the same frames through Rust NSH bridge.
- `input`: framed JSON `{type:"move",x,y}`, `{type:"button",button:"left"|"right"|"middle",down}`, `{type:"scroll",dx,dy}`, `{type:"key",key,down}`, `{type:"text",text}`. Coordinates bounded by selected display; release held input on stream/session close. Input permission is host-side, never granted by relay alone.
- `shell`: OPEN `{cols:80,rows:24}`; raw PTY bytes both directions. Resize through control with stream ID. Terminal process runs under host user. Local `--allow-shell` required; no arbitrary command strings generated by relay. Exit status emitted as a control notification. Shutdown kills child and closes PTY.
- `file.get` on file profile: OPEN `{path}`; raw download bytes then FIN, peer closes request direction with FIN.
- `file.put`: OPEN `{path,size}`; raw upload bytes, exact byte count then FIN; host sends framed JSON receipt `{size,sha256}` then FIN. Uploads use a temporary file and atomic commit, reject existing destination; interrupted uploads removed. `--allow-write` required. All access relative to explicit `--share-dir` using capability-safe filesystem handles, including symlink escape/TOCTOU defense.
- Desktop and input require desktop build plus local permissions; clipboard requires `--allow-clipboard`; files require share root. Control reports available capabilities. No automatic clipboard reads on connect. Session/application errors are visible to clients rather than fake successful no-op responses.

TMP Session calls have one owner. Dedicated byte read/write progress prevents bounded pipe deadlocks. All receive.outbound precedes event-induced packets in the FIFO. App queues and record assemblers have explicit limits; consume returns credit only after bounded acceptance or processing. Sending retries after every packet including WINDOW. FIN closes one direction; RESET cancels its stream; GOAWAY refuses new streams and drains accepted streams while preserving unread accounting. Carrier failure cancels all workers, kills PTYs and releases input. No transparent input replay.

## Acceptance gates

1. Self-hosted relay → two independently encrypted endpoints → real TMP streams; wrong operator/role tokens, invalid crypto, replay, duplicate endpoint and frame overflow rejected.
2. Browser controller completes real shell input/output, file list/upload/download and screenshots/desktop when a supported display is available. Agent fixtures cannot masquerade as physical capture.
3. Headless integration test runs real PTY/files without a desktop. Test traversal, symlink escape, overwrite, partial upload cleanup, child termination and credit stalls.
4. Web and native builds have distinct verified commands. Windows/macOS artifacts must pass their actual CI builds; no Linux-only cross compilation claim. Headless archives and relay Docker image have deterministic artifact names and checksums. Release workflow is tag-driven; unsigned builds labeled as such when signing secrets absent.
5. Cloudflare bundle typechecks and local Wrangler/test harness validates DO WebSocket pairing. Actual account deployment is reported separately from config readiness.
6. Independent design/implementation review before publishing. Record supported/unsupported platform backends honestly.


## v0.1 threat model and scope decisions

Implementation decisions under the user's delegated architecture authority:

- The invitation authenticates possession of a high-entropy, per-room bearer PSK. It does not establish persistent device identity, durable pairing, or forward secrecy. Treat invitations as credentials; disconnect consumes the room and reconnection requires a new host-created room and invitation. The controller never receives the operator token. No automatic retry of input, shell bytes, or destructive operations.
- Relay blindness assumes authentic client software and uncompromised endpoints. For an untrusted relay, deliver the web client from a separate trusted HTTPS origin. A relay serving modified JavaScript can read fragment credentials. Timing, sizes and room metadata remain visible. Serving web assets alongside the Rust relay is a self-hosted convenience for operators trusted to deliver the client.
- Experimental JPEG desktop and other services share one reliable ordered link. A file transfer can delay input/media. No interactive latency or seamless migration guarantee is made. Independent media/control/bulk links require a future version of the application session contract, without extending TMP.
- Browser bundling imports the pinned released SDK's installed session/framing entry files directly to exclude Node-only transport imports; it does not maintain a second TMP state machine.

## Runtime budgets and lifecycle details

The TMP runtime has exactly one Session owner. Independent reader and writer tasks preserve partial carrier progress. Per connection: 32 active streams, 128 outbound packets, 32 decoded inbound packets, 128 application commands, 8 unhandled remote opens. Each stream has a 64KiB duplex capacity in each direction, 4 queued inbound bridge chunks, one coalesced pending input buffer bounded by the negotiated receive window, and at most one pending outbound chunk. Valid tiny DATA bursts are coalesced rather than rejected by packet count; FIN stays after queued bytes. Outbound chunk size is min(16KiB, peer max_data, peer stream_window). Packet encoding is bounded to 131072 bytes plus prefix; encrypted carrier max150000. Queue overflow closes the affected stream or connection explicitly. The actor never awaits outbound queue capacity while holding up incoming WINDOW processing; every packet, including WINDOW without events, resumes sending. Eligible streams send at most one chunk per pump pass, rotating the starting stream after each successful send. Cancelled or timed-out pending OPEN requests are reclaimed by one-second maintenance. A separate writer-progress counter prevents queued-but-unwritten DATA/FIN from becoming successful closure. The runtime sends heartbeat PING every10 seconds and fails after30 seconds without its PONG. GOAWAY drains for at most60 seconds, then fails explicitly; a TMP PING/PONG barrier confirms relay forwarding before closing the WebSocket.

Application services additionally enforce a 16-stream default concurrency limit (maximum64), 60-second idle I/O deadlines, max8MiB application record, max1MiB control record, 4096-byte OPEN metadata, and host-configured file size limit (default1GiB). A record larger than the TMP stream window is consumed incrementally as bytes enter the bounded application pipe; complete record assembly does not hold TMP credit. Memory remains bounded by the stated per-stream/service limits; these are not a global OS memory quota.

Local EOF closes the sending half only. Remote FIN closes the receiving half only after queued bytes. A host service reports successful completion separately from pipe EOF so permission/commit failures send RESET instead of a successful FIN. Controllers must validate terminal stream status after download EOF. RESET and carrier loss are errors, never a successful empty download. GOAWAY drains accepted streams; application shutdown/carrier failure cancels workers. No upload is acknowledged until exact-size validation, verified request FIN, and atomic no-overwrite commit complete. Host adapters use the error-aware MuxStream I/O boundary and independently cancel service futures on RESET/carrier loss, including after input FIN. The atomic commit is the race boundary: a cancellation processed after commit cannot roll it back.

Rust relay defaults: 128 rooms, 8-record destination queues, 30-second waiting timeout, 1-hour expiry (hard max24h), write deadlines. Cloudflare's WebSocket API exposes no drain signal; its conservative cumulative session forwarding limit defaults64MiB, configurable up to256MiB, in addition to pre-pair queue limits. Quota exhaustion closes the room. This explicit limit prevents claiming unbounded bulk transfer on Workers.
