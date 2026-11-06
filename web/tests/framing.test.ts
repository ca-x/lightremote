import { test } from "node:test";
import assert from "node:assert/strict";
import { Records, frame } from "../src/records.ts";
import { Session } from "../node_modules/@lib-x/tmp/dist/session.js";
import {
  encode,
  IncrementalDecoder,
} from "../node_modules/@lib-x/tmp/dist/framing.js";
const enc = new TextEncoder();
function concat(...chunks: Uint8Array[]) {
  const bytes = new Uint8Array(chunks.reduce((n, c) => n + c.length, 0));
  let at = 0;
  for (const c of chunks) {
    bytes.set(c, at);
    at += c.length;
  }
  return bytes;
}
test("application framing accepts arbitrary splits, coalescing, zero-length records and async processing", async () => {
  const data = concat(
    frame(enc.encode("one")),
    frame(new Uint8Array()),
    frame(enc.encode("three")),
  );
  for (let split = 1; split <= data.length; split++) {
    const records = new Records(64),
      output: string[] = [];
    for (let i = 0; i < data.length; i += split)
      await records.feed(data.subarray(i, i + split), async (body) => {
        await Promise.resolve();
        output.push(new TextDecoder().decode(body));
      });
    records.finish();
    assert.deepEqual(output, ["one", "", "three"]);
  }
});
test("record limits checked at header and truncated body/header fail at FIN", async () => {
  await assert.rejects(
    new Records(4).feed(frame(new Uint8Array(5)).subarray(0, 4), () => {}),
    /limit/,
  );
  for (const n of [1, 2, 3, 4, 5, 6]) {
    const records = new Records();
    await records.feed(frame(enc.encode("hey")).subarray(0, n), () => {});
    assert.throws(() => records.finish(), /inside/);
  }
});
test("actual SDK TMP framing accepts split and coalesced Hello packets", () => {
  const a = new Session("initiator"),
    b = new Session("initiator");
  const packets = [a.start(), b.start()];
  const data = concat(...packets.map(encode));
  for (const split of [1, 2, 3, 5, 17, data.length]) {
    const output: unknown[] = [],
      decoder = new IncrementalDecoder((p) => output.push(p));
    for (let i = 0; i < data.length; i += split)
      decoder.feed(data.subarray(i, i + split));
    decoder.finish();
    assert.deepEqual(output, packets);
  }
});
