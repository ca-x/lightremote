# roc-remote relay

`roc-remote-relay` is a separate executable with explicit dependencies. It forwards opaque binary records between exactly one host and one controller. It cannot grant desktop, shell or filesystem permissions. The encrypted hello is itself a binary record and may arrive before the other endpoint connects.

From the repository root:

```sh
cargo build -p roc-remote-relay
export ROC_REMOTE_RELAY_TOKEN="$(openssl rand -hex 32)"
ROC_REMOTE_WEB_DIR=web/dist cargo run -p roc-remote-relay
./deploy/smoke-relay.sh ./target/debug/roc-remote-relay
```

The executable refuses to start without a nonempty operator token. It never logs tickets, payloads, Authorization headers or request URLs. Do not enable URL/header logging in a reverse proxy; role tickets occur in the WebSocket protocol request header. Protect public deployments with TLS at the reverse proxy.

| Environment variable | Default | Meaning |
| --- | --- | --- |
| `ROC_REMOTE_RELAY_TOKEN` | mandatory | Operator Bearer token for room creation; use a random high-entropy secret |
| `ROC_REMOTE_BIND` | `0.0.0.0:8787` | Listening address |
| `ROC_REMOTE_MAX_ROOMS` | `128` | Live rooms, including unused rooms; range 1–4096 |
| `ROC_REMOTE_ROOM_TTL_SECONDS` | `3600` | Absolute lifetime, range 1–86400 seconds |
| `ROC_REMOTE_WAIT_SECONDS` | `30` | Time after first join to pair, range 1–86400 seconds |
| `ROC_REMOTE_ALLOWED_ORIGINS` | empty | Comma-separated exact HTTP(S) browser origins; when populated replaces the default origin policy |
| `ROC_REMOTE_PUBLIC_ORIGIN` | request scheme/Host | Canonical relay origin, required behind TLS termination for same-origin browser access |
| `ROC_REMOTE_WEB_DIR` | disabled | Optional static directory, typically `web/dist`; serves its index at `/` |

An absent Origin is allowed for native clients, which still need role tickets. By default browser Origin must match the relay origin or be `localhost`, `127.0.0.1`, or `[::1]` on any port. `Origin: null`, malformed origins, wildcards and non-HTTP(S) origins are rejected. To restrict production clients, configure an explicit allowlist; loopback then has no implicit exemption. Reverse proxy forwarded headers are not trusted: configure `ROC_REMOTE_PUBLIC_ORIGIN` to the external HTTPS origin.

Serving the browser client on the relay origin is convenient. A malicious or compromised relay serving that JavaScript can modify the client and steal the invitation fragment/encryption secret. Prefer serving a separately trusted, versioned client from an independent HTTPS origin, and allowlist that exact origin. End-to-end encryption protects against a relay that only forwards ciphertext; it cannot protect a client whose code the relay controls. Keep the operator token in trusted provisioning tooling; do not distribute it with a public browser bundle.

## HTTP and WebSocket contract

- `GET /healthz` returns `{"ok":true,"protocol":"lightremote-relay.v1"}`.
- `POST /v1/rooms` with `Authorization: Bearer <operator-token>` and JSON `{}` returns `{room_id,host_token,client_token,expires_at}`. `expires_at` is Unix seconds. Tickets use 32 random bytes encoded as 43-character unpadded base64url. Creation body is limited to 1024 bytes; room capacity exhaustion returns 429. Unauthenticated creation returns 401.
- Upgrade `GET /v1/rooms/<room_id>/ws` with protocols `lightremote.v1, host.<host_token>` or `lightremote.v1, client.<client_token>`. The selected protocol is **only** `lightremote.v1`; tickets are never reflected. Exactly these two protocol offers are required. Query strings are rejected, including query-based tickets. Wrong/missing role credentials return 401, unknown/consumed rooms 404, duplicate authenticated roles 409.
- Records are binary only, at most 150000 bytes per complete WebSocket message, including fragmented messages. Text or overflow closes and consumes the room. Control ping/pong frames are handled by the WebSocket transport and never forwarded as application records.
- Each destination has an eight-record queue, at most 1,200,000 queued payload bytes. Before that destination joins, overflow consumes the room. After pairing, each source may retain one additional pending record and pauses inbound reads until its peer queue has capacity. While waiting, it continues draining its own destination queue, allowing bidirectional bursts and flow-control traffic to make progress without a circular wait. Pending forwarding and each socket write must finish within five seconds; the pending deadline also bounds any intervening write. A close, read/write failure, upgrade failure after role reservation, waiting timeout or absolute expiry closes both ends and destroys the room. Timers run without incoming traffic. Room tickets cannot reconnect; obtain a fresh room and encryption secret.

The Rust relay keeps room state in memory. Process restart invalidates every room. Room creation and role reservation are serialized under one short mutex; no lock spans network I/O. Queue memory is bounded by configured room count, two destination queues, at most one pending forwarding record per socket, and one active write/read per socket. Cancelled rooms also cancel their expiry/wait timer tasks.

## Verification

```sh
cargo fmt --manifest-path crates/relay/Cargo.toml -- --check
cargo clippy -p roc-remote-relay --all-targets -- -D warnings
cargo test -p roc-remote-relay
cargo doc -p roc-remote-relay --no-deps
```

Integration tests bind real ephemeral TCP listeners and perform HTTP creation and actual WebSocket upgrades. They cover operator authentication, room quotas, origin restrictions, role isolation, protocol selection, duplicate rejection, pre-peer forwarding/overflow, bidirectional forwarding at the size boundary, simultaneous 512 KiB bursts with TCP_NODELAY on one and multiple runtime workers, stalled paired-reader deadlines, oversized/text messages, teardown, timer expiry and capacity recovery. `deploy/smoke-relay.sh` starts the executable using environment configuration and validates health, auth and creation; it accepts a binary path to avoid changing Cargo locks.

See `../../deploy/README.md` for Docker and `../../cloudflare/README.md` for the alternate Workers relay and its finite session transfer limit.
