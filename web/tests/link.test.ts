import { test } from "node:test";
import assert from "node:assert/strict";
import { Link } from "../src/link.ts";
import { Records, parseJSON } from "../src/records.ts";
import { HostFixture } from "./host-fixture.ts";
test("reusable Node Link: FIN half-close preserves download, explicit consume drives WINDOW credit", async () => {
  const fixture = await HostFixture.start();
  let link: Link | undefined;
  try {
    link = await Link.connect(fixture.invite());
    const stream = await link.open("file.get", { path: "large.bin" });
    await stream.end();
    const first = await stream.read();
    assert.ok(first);
    assert.ok(first.bytes.length);
    const windowsBefore = fixture.windows;
    await new Promise((resolve) => setTimeout(resolve, 25));
    assert.equal(
      fixture.windows,
      windowsBefore,
      "No WINDOW before application consumption",
    );
    const chunks = [first.bytes];
    first.consume();
    first.consume(); // idempotent acknowledgement
    await stream.process((bytes) => {
      chunks.push(bytes);
    });
    assert.deepEqual(
      Buffer.concat(chunks),
      Buffer.from(fixture.files.get("large.bin")!),
    );
    assert.ok(fixture.windows > 20);
    assert.ok(fixture.remoteFins >= 1);
    assert.deepEqual(fixture.errors, []);
  } finally {
    link?.close();
    await fixture.close();
  }
});
test("Node Link upload progresses with tiny peer windows and response is readable after local FIN", async () => {
  const fixture = await HostFixture.start();
  let link: Link | undefined;
  try {
    link = await Link.connect(fixture.invite());
    const stream = await link.open("file.put", {
      path: "node-upload.bin",
      size: 300000,
    });
    const records = new Records(1024 * 1024);
    let receipt: any;
    const incoming = stream.process((bytes) =>
      records.feed(bytes, (body) => {
        receipt = parseJSON(body);
      }),
    );
    void incoming.catch(() => {});
    for (let i = 0; i < 300000; i += 60000)
      await stream.write(new Uint8Array(60000).fill(77));
    await stream.end();
    await incoming;
    records.finish();
    assert.equal(receipt.size, 300000);
    assert.match(receipt.sha256, /^[0-9a-f]{64}$/);
    assert.deepEqual(
      fixture.files.get("node-upload.bin"),
      Buffer.alloc(300000, 77),
    );
    assert.deepEqual(fixture.errors, []);
  } finally {
    link?.close();
    await fixture.close();
  }
});
test("RESET cancels a credit-stalled write and pending read; per-stream send memory is bounded", async () => {
  const fixture = await HostFixture.start();
  let link: Link | undefined;
  try {
    link = await Link.connect(fixture.invite());
    const stream = await link.open("shell", { cols: 80, rows: 24 });
    const prompt = await stream.read();
    prompt!.consume();
    fixture.withholdCredit = true;
    await assert.rejects(
      stream.write(new Uint8Array(128 * 1024 + 1)),
      /queue is full/,
    );
    const pending = stream.write(new Uint8Array(65536));
    const writeRejected = assert.rejects(pending, /test reset/);
    const readRejected = assert.rejects(stream.read(), /test reset/);
    stream.reset("test reset");
    await writeRejected;
    await readRejected;
    assert.equal(stream.writes.length, 0);
  } finally {
    link?.close();
    await fixture.close();
  }
});
test("connect cancellation closes an in-progress carrier and does not leave a live session", async () => {
  const fixture = await HostFixture.start();
  try {
    const abort = new AbortController();
    const connecting = Link.connect(fixture.invite(), () => {}, abort.signal);
    const rejected = assert.rejects(connecting, /cancelled/);
    abort.abort();
    await rejected;
    await assert.rejects(
      Link.connect(fixture.invite(), () => {}, abort.signal),
      /cancelled/,
    );
  } finally {
    await fixture.close();
  }
});
