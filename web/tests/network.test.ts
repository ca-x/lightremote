import { test, type TestContext } from "node:test";
import assert from "node:assert/strict";
import { setImmediate as immediate } from "node:timers/promises";
import { Link, type LinkOptions } from "../src/link.ts";
import { HostFixture } from "./host-fixture.ts";
import { base64url, type Invite } from "../src/secure.ts";

const budgets: Partial<LinkOptions> = {
  connectTimeoutMs: 1000,
  openTimeoutMs: 200,
  sendTimeoutMs: 500,
  heartbeatIntervalMs: 100,
  heartbeatTimeoutMs: 200,
  degradedAfterMs: 50,
};
function clock(t: TestContext) {
  let now = 0;
  t.mock.timers.enable({ apis: ["setTimeout"] });
  t.mock.method(performance, "now", () => now);
  const schedule = globalThis.setTimeout, cancel = globalThis.clearTimeout;
  const timers = new Set<ReturnType<typeof setTimeout>>();
  t.mock.method(globalThis, "setTimeout", ((callback: (...args: unknown[]) => void, delay?: number, ...args: unknown[]) => {
    const timer = schedule(() => { timers.delete(timer); callback(...args); }, delay);
    timers.add(timer);
    return timer;
  }) as typeof setTimeout);
  t.mock.method(globalThis, "clearTimeout", ((timer: ReturnType<typeof setTimeout>) => {
    timers.delete(timer);
    cancel(timer);
  }) as typeof clearTimeout);
  return { timers, advance(ms: number) { now += ms; t.mock.timers.tick(ms); } };
}
async function until(check: () => boolean) {
  const end = process.hrtime.bigint() + 3_000_000_000n;
  while (!check()) {
    if (process.hrtime.bigint() > end) throw new Error("Network condition timed out");
    await immediate();
  }
}
async function connect(t: TestContext, time: ReturnType<typeof clock>, ...args: Parameters<typeof Link.connect>) {
  const link = await Link.connect(...args);
  // Node's native write callbacks run on the real loop while polling uses our fake
  // clock. Use an empty browser queue until each test installs its controlled buffer.
  const socket = (link as unknown as { socket: WebSocket }).socket;
  t.mock.getter(socket, "bufferedAmount", () => 0);
  time.advance(10);
  await immediate();
  return link;
}
class BlackholeSocket {
  static instances: BlackholeSocket[] = [];
  binaryType = "arraybuffer";
  protocol = "lightremote.v1";
  bufferedAmount = 0;
  onmessage = null;
  onopen = null;
  onerror = null;
  onclose = null;
  closes = 0;
  constructor() { BlackholeSocket.instances.push(this); }
  close = () => { this.closes++; };
}
const invite: Invite = { relay: "http://127.0.0.1:1", room_id: "test_room", client_token: "test_ticket", secret: base64url(new Uint8Array(32)) };

test("connection budget bounds a blackhole and never reconnects the invitation", async t => {
  const time = clock(t);
  BlackholeSocket.instances = [];
  t.mock.method(globalThis, "WebSocket", BlackholeSocket);
  let closed = 0;
  const pending = Link.connect(invite, () => { closed++; }, undefined, { connectTimeoutMs: 40 });
  const rejected = assert.rejects(pending, /handshake timed out/);
  time.advance(40);
  await rejected;
  assert.equal(time.timers.size, 0, "closed negotiation retained a deadline");
  time.advance(600_000);
  assert.equal(BlackholeSocket.instances.length, 1);
  assert.equal(BlackholeSocket.instances[0].closes, 1);
  assert.equal(closed, 1);
});

test("abort cancels negotiation, removes its listener and clears its deadline", async t => {
  const time = clock(t);
  BlackholeSocket.instances = [];
  t.mock.method(globalThis, "WebSocket", BlackholeSocket);
  const controller = new AbortController();
  const remove = t.mock.method(controller.signal, "removeEventListener");
  const pending = Link.connect(invite, undefined, controller.signal);
  const rejected = assert.rejects(pending, /cancelled/);
  controller.abort();
  await rejected;
  assert.equal(remove.mock.callCount(), 1);
  assert.equal(time.timers.size, 0, "cancelled negotiation retained a deadline");
  time.advance(600_000);
  assert.equal(BlackholeSocket.instances[0].closes, 1);
  await assert.rejects(Link.connect(invite, undefined, controller.signal), /cancelled/);
  assert.equal(BlackholeSocket.instances.length, 1);
});

test("invalid budgets fail before consuming a relay ticket", async t => {
  BlackholeSocket.instances = [];
  t.mock.method(globalThis, "WebSocket", BlackholeSocket);
  for (const value of [0, -1, NaN, Infinity, 300001])
    await assert.rejects(Link.connect(invite, undefined, undefined, { sendTimeoutMs: value }), /budgets/);
  assert.equal(BlackholeSocket.instances.length, 0);
});

test("unanswered end-to-end heartbeat degrades then fails active streams without retry", async t => {
  const fixture = await HostFixture.start();
  fixture.holdPongs = true;
  const time = clock(t);
  let link: Link | undefined;
  let reason: Error | undefined;
  try {
    link = await connect(t, time, fixture.invite(), error => { reason = error; }, undefined, budgets);
    const stream = await link.open("shell");
    const phases: string[] = [];
    link.onHealth = health => phases.push(health.phase);
    time.advance(100);
    await until(() => fixture.pings === 1);
    const internal = link as unknown as { heartbeat: { token: bigint }; event(event: { type: "pong"; token: bigint }): void };
    internal.event({ type: "pong", token: internal.heartbeat.token ^ 1n });
    time.advance(50);
    assert.equal(link.health.phase, "degraded");
    assert.equal(link.health.rtt_ms, null);
    time.advance(150);
    assert.match(reason?.message ?? "", /heartbeat timed out/);
    assert.equal(link.health.phase, "closed");
    await assert.rejects(stream.write(new Uint8Array([1])), /heartbeat timed out/);
    time.advance(600_000);
    assert.equal(fixture.pings, 1);
    assert.deepEqual(phases, ["degraded", "closed"]);
  } finally {
    link?.close();
    await fixture.close();
  }
});

test("abort after connect cancels a blocked sender and pending OPEN and read", async t => {
  const fixture = await HostFixture.start();
  const time = clock(t);
  let link: Link | undefined;
  try {
    const controller = new AbortController();
    link = await connect(t, time, fixture.invite(), undefined, controller.signal, budgets);
    const stream = await link.open("shell");
    await until(() => stream.outstanding > 0);
    (await stream.read())!.consume();
    const read = assert.rejects(stream.read(), /cancelled/);
    const socket = (link as unknown as { socket: WebSocket }).socket;
    t.mock.getter(socket, "bufferedAmount", () => 512 * 1024);
    const open = assert.rejects(link.open("shell"), /cancelled/);
    controller.abort();
    await Promise.all([read, open]);
    assert.equal(link.health.phase, "closed");
    await fixture.close();
    assert.equal(time.timers.size, 0, "closed link retained a heartbeat, OPEN or send polling timer");
    time.advance(600_000);
  } finally {
    link?.close();
    if (fixture.server.address()) await fixture.close();
  }
});

test("matching PONG reports RTT, permits the next probe and recovers health", async t => {
  const fixture = await HostFixture.start();
  fixture.holdPongs = true;
  const time = clock(t);
  let link: Link | undefined;
  try {
    link = await connect(t, time, fixture.invite(), undefined, undefined, budgets);
    time.advance(100);
    await until(() => fixture.pings === 1);
    time.advance(50);
    assert.equal(link.health.phase, "degraded");
    fixture.releasePongs();
    await until(() => link!.health.rtt_ms !== null);
    assert.equal(link.health.rtt_ms, 50);
    fixture.holdPongs = false;
    time.advance(100);
    await until(() => fixture.pings === 2 && link!.health.phase === "healthy");
    assert.equal(link.health.rtt_ms, 0);
    link.close();
    time.advance(600_000);
    assert.equal(fixture.pings, 2);
  } finally {
    link?.close();
    await fixture.close();
  }
});

test("send backpressure uses a deadline, fails pending OPEN and clears polling", async t => {
  const fixture = await HostFixture.start();
  const time = clock(t);
  let link: Link | undefined;
  let reason: Error | undefined;
  try {
    const controller = new AbortController();
    link = await connect(t, time, fixture.invite(), error => { reason = error; }, controller.signal,
      { ...budgets, openTimeoutMs: 1000, sendTimeoutMs: 300 });
    const socket = (link as unknown as { socket: WebSocket }).socket;
    t.mock.getter(socket, "bufferedAmount", () => 512 * 1024);
    const pending = link.open("shell");
    const rejected = assert.rejects(pending, /send timed out/);
    time.advance(100);
    time.advance(200);
    await rejected;
    assert.match(reason?.message ?? "", /send timed out/);
    assert.equal(fixture.pings, 0, "probe waiting behind carrier was sent");
    assert.equal(link.health.phase, "closed");
    controller.abort();
    time.advance(600_000);
    assert.match(reason?.message ?? "", /send timed out/, "abort overwrote terminal reason");
  } finally {
    link?.close();
    await fixture.close();
  }
});

test("probe waiting in a slow send queue does not start its PONG budget", async t => {
  const fixture = await HostFixture.start();
  fixture.holdPongs = true;
  const time = clock(t);
  let link: Link | undefined;
  let reason: Error | undefined;
  try {
    link = await connect(t, time, fixture.invite(), error => { reason = error; }, undefined, budgets);
    const socket = (link as unknown as { socket: WebSocket }).socket;
    let buffered = 512 * 1024;
    t.mock.getter(socket, "bufferedAmount", () => buffered);
    time.advance(100);
    time.advance(250);
    assert.equal(reason, undefined, "queue time incorrectly expired the PONG budget");
    assert.equal(fixture.pings, 0);
    buffered = 0;
    time.advance(10);
    await until(() => fixture.pings === 1);
    time.advance(50);
    assert.equal(link.health.phase, "degraded");
    fixture.releasePongs();
    await until(() => link!.health.rtt_ms !== null);
    assert.equal(link.health.rtt_ms, 50, "RTT included carrier queue residence");
  } finally {
    link?.close();
    await fixture.close();
  }
});

test("a one-byte browser buffer after send retains the absolute write deadline", async t => {
  const fixture = await HostFixture.start();
  const time = clock(t);
  let link: Link | undefined;
  let reason: Error | undefined;
  try {
    link = await connect(t, time, fixture.invite(), error => { reason = error; }, undefined,
      { ...budgets, heartbeatIntervalMs: 1000, openTimeoutMs: 1000, sendTimeoutMs: 80 });
    const socket = (link as unknown as { socket: WebSocket }).socket;
    let buffered = 0, sends = 0;
    t.mock.getter(socket, "bufferedAmount", () => buffered);
    t.mock.method(socket, "send", () => { buffered = 1; sends++; });
    const pending = link.open("shell");
    const rejected = assert.rejects(pending, /send timed out/);
    await until(() => sends === 1);
    time.advance(79);
    assert.equal(reason, undefined);
    time.advance(1);
    assert.match((reason as Error | undefined)?.message ?? "", /send timed out/);
    await rejected;
    assert.equal(link.health.phase, "closed");
    await fixture.close();
    assert.equal(time.timers.size, 0);
    time.advance(600_000);
    assert.equal(sends, 1, "closed sender retried the queued operation");
  } finally {
    link?.close();
    if (fixture.server.address()) await fixture.close();
  }
});

test("PONG budget and RTT start only after the browser buffer drains", async t => {
  const fixture = await HostFixture.start();
  fixture.holdPongs = true;
  const time = clock(t);
  let link: Link | undefined;
  let reason: Error | undefined;
  try {
    link = await connect(t, time, fixture.invite(), error => { reason = error; }, undefined, budgets);
    const socket = (link as unknown as { socket: WebSocket }).socket;
    const send = socket.send.bind(socket);
    let buffered = 0;
    const records: Parameters<WebSocket["send"]>[0][] = [];
    t.mock.getter(socket, "bufferedAmount", () => buffered);
    t.mock.method(socket, "send", (data: Parameters<WebSocket["send"]>[0]) => { buffered = 1; records.push(data); });
    time.advance(100);
    await until(() => records.length === 1);
    time.advance(250);
    assert.equal(reason, undefined, "browser queue time expired the PONG budget");
    assert.equal(link.health.rtt_ms, null);
    assert.equal(fixture.pings, 0);
    buffered = 0;
    send(records[0]);
    time.advance(10);
    await until(() => fixture.pings === 1);
    // Let the resumed sender observe zero before advancing its new PONG clock.
    await immediate();
    time.advance(50);
    assert.equal(reason, undefined);
    fixture.releasePongs();
    await until(() => link!.health.rtt_ms !== null);
    assert.equal(link.health.rtt_ms, 50, "RTT included browser buffer residence");
  } finally {
    link?.close();
    await fixture.close();
  }
});

test("close cancels browser drain waiting and clears the retained write deadline", async t => {
  const fixture = await HostFixture.start();
  const time = clock(t);
  let link: Link | undefined;
  let closes = 0;
  try {
    link = await connect(t, time, fixture.invite(), () => { closes++; }, undefined, budgets);
    const socket = (link as unknown as { socket: WebSocket }).socket;
    let buffered = 0, sends = 0;
    t.mock.getter(socket, "bufferedAmount", () => buffered);
    t.mock.method(socket, "send", () => { buffered = 1; sends++; });
    const rejected = assert.rejects(link.open("shell"), /Disconnected/);
    await until(() => sends === 1);
    link.close();
    await rejected;
    await fixture.close();
    assert.equal(time.timers.size, 0, "close retained a drain poll or write deadline");
    time.advance(600_000);
    await immediate();
    assert.equal(time.timers.size, 0, "cancelled drain wait scheduled another timer");
    assert.equal(sends, 1);
    assert.equal(closes, 1);
  } finally {
    link?.close();
    if (fixture.server.address()) await fixture.close();
  }
});

test("a matching PONG arriving before browser drain is retained until the barrier", async t => {
  const fixture = await HostFixture.start();
  const time = clock(t);
  let link: Link | undefined;
  try {
    link = await connect(t, time, fixture.invite(), undefined, undefined, budgets);
    const socket = (link as unknown as { socket: WebSocket }).socket;
    const send = socket.send.bind(socket);
    let buffered = 0;
    t.mock.getter(socket, "bufferedAmount", () => buffered);
    t.mock.method(socket, "send", (data: Parameters<WebSocket["send"]>[0]) => { send(data); buffered = 1; });
    const events = t.mock.method(link as unknown as { event(event: { type: string }): void }, "event");
    time.advance(100);
    await until(() => events.mock.calls.some(call => call.arguments[0].type === "pong"));
    assert.equal(link.health.rtt_ms, null, "PONG bypassed the browser drain barrier");
    time.advance(250);
    buffered = 0;
    time.advance(10);
    await until(() => link!.health.rtt_ms !== null);
    assert.equal(link.health.rtt_ms, 0, "early reply included browser queue residence");
    assert.equal(link.health.phase, "healthy");
  } finally {
    link?.close();
    await fixture.close();
  }
});
