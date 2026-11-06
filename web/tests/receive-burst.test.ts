import { test } from 'node:test';
import assert from 'node:assert/strict';
import { Link, MuxStream } from '../src/link.ts';

function stream() {
  let reset = 0, consumed = 0;
  const owner = {
    retire() {},
    consume(_stream: MuxStream, count: number) { consumed += count; },
    reset(stream: MuxStream, reason: string) { reset++; stream.fail(new Error(reason)); },
  } as unknown as Link;
  return { io: new MuxStream(1, owner), reset: () => reset, consumed: () => consumed };
}

test('credited tiny receive burst preserves all bytes and genuine FIN without packet-count reset', async () => {
  const { io, reset, consumed } = stream();
  const expected = Uint8Array.from({ length: 4096 }, (_, i) => i % 251);
  for (const value of expected) io.receive(Uint8Array.of(value), false);
  io.receive(new Uint8Array(), true);
  assert.equal(reset(), 0);
  assert.equal(io.outstanding, expected.length);
  const result: Uint8Array[] = [];
  await io.process(bytes => { result.push(bytes); });
  assert.deepEqual(Buffer.concat(result), Buffer.from(expected));
  assert.equal(consumed(), expected.length);
  assert.equal(io.outstanding, 0);
  assert.equal(io.remoteFinConsumed, true);
});

test('buffering a later burst preserves already-delivered bytes and applies FIN after queued input', async () => {
  const { io, reset, consumed } = stream();
  io.receive(Uint8Array.of(253), false);
  const first = (await io.read())!;
  const expected = Uint8Array.from({ length: 1024 }, (_, i) => i % 251);
  for (const value of expected) io.receive(Uint8Array.of(value), false);
  io.receive(new Uint8Array(), true);
  assert.equal(reset(), 0);
  assert.deepEqual(first.bytes, Uint8Array.of(253));
  assert.equal(first.fin, false);
  assert.equal(io.remoteFinConsumed, false);
  first.consume(); first.consume();
  const result: Uint8Array[] = [];
  await io.process(bytes => { result.push(bytes); });
  assert.deepEqual(Buffer.concat(result), Buffer.from(expected));
  assert.equal(consumed(), expected.length + 1);
  assert.equal(io.remoteFinConsumed, true);
});

test('receive byte window remains enforced after a valid burst', () => {
  const { io, reset } = stream();
  io.receive(new Uint8Array(128 * 1024), false);
  assert.equal(reset(), 0);
  io.receive(Uint8Array.of(1), false);
  assert.equal(reset(), 1);
});
