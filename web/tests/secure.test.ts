import { test } from "node:test";
import assert from "node:assert/strict";
import { hkdfSync, createCipheriv } from "node:crypto";
import {
  Cipher,
  base64url,
  makeHello,
  parseHello,
  parseInvite,
  websocketURL,
} from "../src/secure.ts";
const enc = new TextEncoder();
const secret = new Uint8Array(32).fill(42);
const client = makeHello("client"),
  host = makeHello("host");
const derive = (
  role: "client" | "host" = "client",
  room = "test-room",
  key = secret,
) => Cipher.derive(key, room, client, host, role);
test("WebCrypto matches independent Node HKDF/AES-GCM, including nonce and AAD", async () => {
  const tx = await derive();
  const info = "lightremote/v1/test-room/client";
  const salt = Buffer.concat([
    Buffer.from(client.nonce, "base64url"),
    Buffer.from(host.nonce, "base64url"),
  ]);
  const key = hkdfSync("sha256", secret, salt, info, 32);
  const nonce = Buffer.alloc(12);
  nonce.writeUInt32BE(1);
  const cipher = createCipheriv("aes-256-gcm", Buffer.from(key), nonce);
  cipher.setAAD(Buffer.from(info));
  const ciphertext = Buffer.concat([
    cipher.update("payload"),
    cipher.final(),
    cipher.getAuthTag(),
  ]);
  const expected = Buffer.concat([Buffer.alloc(8), ciphertext]);
  assert.deepEqual(Buffer.from(await tx.seal(enc.encode("payload"))), expected);
});
test("authenticates payload, secret, room, direction and exact counter; failed records return no plaintext", async () => {
  const tx = await derive(),
    rx = await derive();
  const first = await tx.seal(enc.encode("first")),
    second = await tx.seal(enc.encode("second"));
  const tampered = Uint8Array.from(first);
  tampered[9] ^= 1;
  await assert.rejects(rx.open(tampered));
  await assert.rejects(rx.open(second), /counter/);
  assert.equal(new TextDecoder().decode(await rx.open(first)), "first");
  await assert.rejects(rx.open(first), /counter/);
  assert.equal(new TextDecoder().decode(await rx.open(second)), "second");
  for (const receiver of [
    await derive("host"),
    await derive("client", "other"),
    await derive("client", "test-room", new Uint8Array(32)),
  ])
    await assert.rejects(receiver.open(first));
  const otherHost = makeHello("host");
  await assert.rejects(
    (
      await Cipher.derive(secret, "test-room", client, otherHost, "client")
    ).open(first),
  );
  await assert.rejects(rx.open(new Uint8Array(23)), /length/);
  await assert.rejects(tx.seal(new Uint8Array(150000)), /large/);
  rx.destroy();
  await assert.rejects(rx.open(second), /closed/);
});
test("both directional counters start at zero and fresh handshakes change ciphertext", async () => {
  const a = await derive("host"),
    b = await derive("client");
  const x = await a.seal(enc.encode("x")),
    y = await b.seal(enc.encode("x"));
  assert.equal(new DataView(x.buffer).getBigUint64(0), 0n);
  assert.equal(new DataView(y.buffer).getBigUint64(0), 0n);
  assert.notDeepEqual(x, y);
  const fresh = await Cipher.derive(
    secret,
    "test-room",
    makeHello("client"),
    makeHello("host"),
    "client",
  );
  assert.notDeepEqual(await fresh.seal(enc.encode("x")), y);
});
test("rejects wrong role, version, malformed and oversized handshakes", () => {
  assert.deepEqual(parseHello(enc.encode(JSON.stringify(host)), "host"), host);
  assert.throws(() => parseHello(enc.encode(JSON.stringify(host)), "client"));
  assert.throws(() =>
    parseHello(enc.encode(JSON.stringify({ ...host, version: 2 })), "host"),
  );
  assert.throws(() =>
    parseHello(enc.encode(JSON.stringify({ ...host, nonce: "YQ" })), "host"),
  );
  assert.throws(() => parseHello(new Uint8Array(1025), "host"));
});
test("invitation fragment and pasted URL; TLS, scoped path and subprotocol-safe ticket validation", () => {
  const invite = {
    relay: "https://relay.example/prefix",
    room_id: "test-room",
    client_token: "ticket_123",
    secret: base64url(secret),
  };
  const fragment = base64url(enc.encode(JSON.stringify(invite)));
  assert.deepEqual(parseInvite(`https://client.example/#${fragment}`), invite);
  assert.deepEqual(parseInvite(fragment), invite);
  assert.equal(
    websocketURL(invite),
    "wss://relay.example/prefix/v1/rooms/test-room/ws",
  );
  for (const patch of [
    { relay: "http://remote.example" },
    { relay: "https://a:b@relay.example" },
    { relay: "https://relay.example/?token=x" },
    { client_token: "bad,ticket" },
    { secret: "YQ" },
    { extra: true },
  ]) {
    assert.throws(() =>
      parseInvite(
        base64url(enc.encode(JSON.stringify({ ...invite, ...patch }))),
      ),
    );
  }
  assert.equal(
    parseInvite(
      base64url(
        enc.encode(
          JSON.stringify({ ...invite, relay: "http://127.0.0.1:9000" }),
        ),
      ),
    ).relay,
    "http://127.0.0.1:9000",
  );
});

test("shared Rust/Web v1 vector: deterministic nonces, HKDF info/AAD, and full counter-zero record", async () => {
  const { readFile } = await import("node:fs/promises");
  const { unbase64url } = await import("../src/secure.ts");
  const vector = JSON.parse(
    await readFile(
      new URL("../../testdata/secure-link-v1.json", import.meta.url),
      "utf8",
    ),
  );
  assert.equal(vector.version, 1);
  assert.equal(vector.counter, 0);
  assert.equal(vector.sender, "client");
  assert.equal(
    vector.info_utf8,
    `lightremote/v1/${vector.room_id}/${vector.sender}`,
  );
  assert.equal(vector.aad_utf8, vector.info_utf8);
  const client = {
    type: "lightremote.hello" as const,
    version: 1 as const,
    role: "client" as const,
    nonce: vector.client_nonce_b64url,
  };
  const host = {
    type: "lightremote.hello" as const,
    version: 1 as const,
    role: "host" as const,
    nonce: vector.host_nonce_b64url,
  };
  const key = unbase64url(vector.secret_b64url);
  assert.deepEqual(
    key,
    Uint8Array.from({ length: 32 }, (_, i) => i),
  );
  assert.deepEqual(
    unbase64url(client.nonce),
    Uint8Array.from({ length: 32 }, (_, i) => i + 32),
  );
  assert.deepEqual(
    unbase64url(host.nonce),
    Uint8Array.from({ length: 32 }, (_, i) => i + 64),
  );
  const tx = await Cipher.derive(
    key,
    vector.room_id,
    client,
    host,
    vector.sender,
  );
  const record = await tx.seal(enc.encode(vector.plaintext_utf8));
  assert.equal(base64url(record), vector.record_b64url);
  assert.equal(new DataView(record.buffer).getBigUint64(0), 0n);
  const rx = await Cipher.derive(
    key,
    vector.room_id,
    client,
    host,
    vector.sender,
  );
  assert.equal(
    new TextDecoder().decode(await rx.open(unbase64url(vector.record_b64url))),
    vector.plaintext_utf8,
  );
  // Independently reconstruct the shared fixture with Node's HKDF and AES-GCM.
  const salt = Buffer.concat([
    unbase64url(client.nonce),
    unbase64url(host.nonce),
  ]);
  const derived = hkdfSync("sha256", key, salt, vector.info_utf8, 32);
  const nonce = Buffer.alloc(12);
  nonce.writeUInt32BE(1);
  const native = createCipheriv("aes-256-gcm", Buffer.from(derived), nonce);
  native.setAAD(Buffer.from(vector.aad_utf8));
  const expected = Buffer.concat([
    Buffer.alloc(8),
    native.update(vector.plaintext_utf8),
    native.final(),
    native.getAuthTag(),
  ]);
  assert.equal(expected.toString("base64url"), vector.record_b64url);
});

// Test-only state fixture reaches the real limits without allocating or encrypting gigabytes.
function keyUsageFixture(
  cipher: Cipher,
  next: bigint,
  totalPlaintext: bigint,
): Cipher {
  const state = cipher as unknown as { next: bigint; totalPlaintext: bigint };
  state.next = next;
  state.totalPlaintext = totalPlaintext;
  return cipher;
}
const RECORD_BUDGET = 1n << 32n;
const BYTE_BUDGET = 64n * 1024n ** 3n;

test("directional record budget allows counter 2^32-1 on seal/open then fails closed", async () => {
  const tx = keyUsageFixture(await derive(), RECORD_BUDGET - 1n, 0n);
  const rx = keyUsageFixture(await derive(), RECORD_BUDGET - 1n, 0n);
  const record = await tx.seal(enc.encode("last"));
  assert.equal(new DataView(record.buffer).getBigUint64(0), RECORD_BUDGET - 1n);
  assert.equal(new TextDecoder().decode(await rx.open(record)), "last");
  await assert.rejects(tx.seal(new Uint8Array()), /usage limit/);
  const overrun = Uint8Array.from(record);
  new DataView(overrun.buffer).setBigUint64(0, RECORD_BUDGET);
  await assert.rejects(rx.open(overrun), /usage limit/);
  // Hitting the budget destroys the key, including for direct Cipher callers.
  assert.equal(Reflect.get(tx, "key"), undefined);
  assert.equal(Reflect.get(rx, "key"), undefined);
});

test("directional byte budget allows exactly 64 GiB then rejects the next plaintext byte on seal/open", async () => {
  const tx = keyUsageFixture(await derive(), 0n, BYTE_BUDGET - 1n);
  const rx = keyUsageFixture(await derive(), 0n, BYTE_BUDGET - 1n);
  const record = await tx.seal(enc.encode("x"));
  assert.equal(new TextDecoder().decode(await rx.open(record)), "x");
  assert.equal(Reflect.get(tx, "totalPlaintext"), BYTE_BUDGET);
  assert.equal(Reflect.get(rx, "totalPlaintext"), BYTE_BUDGET);
  const nextRecord = await keyUsageFixture(await derive(), 1n, 0n).seal(
    enc.encode("y"),
  );
  await assert.rejects(tx.seal(enc.encode("y")), /usage limit/);
  await assert.rejects(rx.open(nextRecord), /usage limit/);
  assert.equal(Reflect.get(tx, "key"), undefined);
  assert.equal(Reflect.get(rx, "key"), undefined);
});

test("an over-budget record is rejected before encrypt/decrypt and unsuccessful authentication uses no byte budget", async () => {
  const tx = keyUsageFixture(await derive(), 0n, BYTE_BUDGET - 1n);
  const rx = keyUsageFixture(await derive(), 0n, BYTE_BUDGET - 1n);
  const record = await (await derive()).seal(enc.encode("xx"));
  await assert.rejects(tx.seal(enc.encode("xx")), /usage limit/);
  await assert.rejects(rx.open(record), /usage limit/);
  assert.equal(Reflect.get(tx, "next"), 0n);
  assert.equal(Reflect.get(rx, "next"), 0n);
  assert.equal(Reflect.get(tx, "totalPlaintext"), BYTE_BUDGET - 1n);
  assert.equal(Reflect.get(rx, "totalPlaintext"), BYTE_BUDGET - 1n);
  const limited = keyUsageFixture(await derive(), 0n, BYTE_BUDGET - 1n);
  const valid = await (await derive()).seal(enc.encode("x"));
  const tampered = Uint8Array.from(valid);
  tampered[8] ^= 1;
  await assert.rejects(limited.open(tampered));
  assert.equal(Reflect.get(limited, "next"), 0n);
  assert.equal(Reflect.get(limited, "totalPlaintext"), BYTE_BUDGET - 1n);
  assert.equal(new TextDecoder().decode(await limited.open(valid)), "x");
});
