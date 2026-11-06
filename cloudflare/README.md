# Cloudflare Workers + Durable Objects relay

This implements the same `/healthz`, authenticated `POST /v1/rooms`, and role-ticket WebSocket API as `crates/relay`. `expires_at` is Unix seconds. Hello messages are binary records. No account deployment is needed to typecheck, bundle or run the local runtime tests.

```sh
npm ci
npm run check
npm test
npm run build
cp .dev.vars.example .dev.vars
# Replace the example with a fresh random operator token.
npm run dev
```

Deploy into your own account only when ready:

```sh
npx wrangler secret put LIGHTREMOTE_RELAY_TOKEN
npm run deploy
```

The token is a mandatory Worker secret, never a checked-in Wrangler variable. Missing token or invalid limits fail closed with HTTP 503. `.dev.vars` is ignored. No real credentials, account ID or live URLs are included. Rotate the secret through Wrangler. Existing role tickets stay valid until their room expires or is consumed; to revoke all rooms, deploy under a fresh DO namespace/migration with a new secret.

The environment variables, Origin semantics, status codes, protocol offers and 150000-byte binary limit are documented in `../crates/relay/README.md`. Set `LIGHTREMOTE_ALLOWED_ORIGINS` for production clients. Same-origin and HTTP(S) loopback are the defaults; absent Origin is allowed for native clients. This Worker serves the relay API only. Serve the browser client from an independently trusted HTTPS origin and allowlist it: a relay that supplies modified client JavaScript can steal encryption secrets, despite encrypted transport.

## Durable ownership and resource bounds

One fixed, named DO owns the bounded room registry and all room sockets. Arbitrary room paths cannot create additional objects. Operator authentication happens before forwarding room creation to the DO and is repeated inside it. A default maximum of 128 rooms includes waiting and unused rooms; the hard configuration ceiling is 4096. No unauthenticated request creates a room, timer or per-room DO. Large creation bodies are streamed with a 1024-byte bound.

`ctx.acceptWebSocket` uses the hibernation API. Each socket attachment persists its room ID, role and schema version. Room ticket hashes, joined flags, absolute expiry, waiting deadline, forwarding budget and pre-peer queue indices live in DO storage. Alarms expire rooms even without new requests. Each destination can queue eight complete records, and records are persisted in chunks of at most 60000 bytes to stay under storage value limits. Live connected traffic is not stored as an unbounded durable log.

Each stateful operation is serialized with `blockConcurrencyWhile`. Durable role reservation precedes socket acceptance. An operation-in-progress marker is persisted before forwarding or queue mutation; normal completion clears it. After isolate restoration, inconsistent role attachments, an incomplete operation or missing socket consumes the room and closes remaining sockets. Restored consistent sockets can continue after hibernation. Consuming removes metadata and queued chunks in one storage transaction before closing sockets, so a crash cannot leave reusable role credentials or orphaned queued records. Stored queues are never replayed after an interrupted operation.

Cloudflare's WebSocket `send()` does **not** provide a drain/backpressure signal. The relay therefore cannot promise endless bounded transfers. `LIGHTREMOTE_SESSION_BYTES` limits total accepted binary bytes across **both directions**, including pre-peer records. The default is **64 MiB (67108864)**; the configurable hard ceiling is **256 MiB (268435456)**. This budget never replenishes when clients receive data. The frame that would exceed it closes both ends and consumes the room. A fresh room, fresh encryption secret and fresh session are required to transfer more data; application transfers do not resume automatically. A separate hard cap of 262144 records per room also bounds buffering of zero-length/tiny messages. These limits bound the amount handed to runtime send queues; they are not measurements of actual socket backlog. They deliberately limit long desktop/file sessions. The Rust relay uses bounded channels and write deadlines and has no cumulative transfer quota.

The singleton simplifies exact capacity limits and unauthenticated allocation resistance, but makes one DO the throughput bottleneck for the deployment. Limits are per deployment; an upstream request-rate limit is still appropriate for Internet abuse protection. Stored queued ciphertext is transient and deleted on consumption. No secrets, tickets, frames or application data are logged by this Worker.

## Verification scope

`npm test` uses the current official `@cloudflare/vitest-plugin` and Cloudflare Workers pool and the local workerd runtime. Tests exercise actual `WebSocketPair` delivery, auth, protocol selection, duplicate role rejection, durable chunked pre-peer queues, forwarding, text rejection, runtime-delivered 150001-byte frames, byte quota exhaustion, overflow, disconnect teardown, alarms and constructor restoration. Hibernation/crash checks reconstruct the DO from real local storage and attachments, including simulated interrupted durable state; they are not live-account process-kill or account-deployment tests. `npm run build` uses Wrangler dry-run and does not publish. Real account deployment, geographically distributed behavior, load ceilings and provider-level crash injection need separate operational validation.

The shared external suite is `../deploy/relay-conformance.py`. From the repository root, `./deploy/test-relay-pair.sh ./target/debug/lightremote-relay` starts the Rust executable and Wrangler locally, runs that exact same suite against both, and terminates both children. It requires a built Rust binary, Python 3, curl, and `npm ci` in this directory.

The pair runner additionally verifies real-clock waiting and absolute-expiration alarms against the local Wrangler runtime. It waits for socket closure before making any HTTP request, covering timer-driven teardown independently of the unit alarm test.
