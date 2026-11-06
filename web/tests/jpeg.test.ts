import { test } from "node:test";
import assert from "node:assert/strict";
import { jpegDimensions, MAX_JPEG_PIXELS } from "../src/jpeg.ts";

// Structurally complete marker fixtures, not compressed pixel data. Tests never decode them.
function jpeg(width: number, height: number, sof = 0xc0): Uint8Array {
  return Uint8Array.from([
    0xff,
    0xd8,
    0xff,
    sof,
    0,
    11,
    8,
    height >> 8,
    height & 255,
    width >> 8,
    width & 255,
    1,
    1,
    0x11,
    0,
    0xff,
    0xda,
    0,
    8,
    1,
    1,
    0,
    0,
    63,
    0,
    0x12,
    0xff,
    0,
    0x34,
    0xff,
    0xd0,
    0x56,
    0xff,
    0xd9,
  ]);
}

test("JPEG marker admission returns dimensions and accepts exact axis/pixel bounds without raster decode", () => {
  assert.deepEqual(jpegDimensions(jpeg(160, 100)), { width: 160, height: 100 });
  assert.deepEqual(jpegDimensions(jpeg(16384, 1024)), {
    width: 16384,
    height: 1024,
  });
  assert.equal(4096 * 4096, MAX_JPEG_PIXELS);
  assert.deepEqual(jpegDimensions(jpeg(4096, 4096, 0xc2)), {
    width: 4096,
    height: 4096,
  });
});

test("tiny oversized SOF fixtures are rejected before any dangerous pixel allocation", () => {
  for (const [width, height] of [
    [16384, 16384],
    [16385, 1],
    [1, 16385],
    [4097, 4096],
    [0, 100],
    [100, 0],
  ]) {
    const bytes = jpeg(width, height);
    assert.ok(bytes.length < 64);
    assert.throws(() => jpegDimensions(bytes), /dimensions/);
  }
});

test("every truncated prefix and malformed marker length is rejected", () => {
  const bytes = jpeg(160, 100);
  for (let n = 0; n < bytes.length; n++)
    assert.throws(() => jpegDimensions(bytes.subarray(0, n)), /JPEG/);
  for (const length of [0, 1, 7, 10, 12, 65535]) {
    const malformed = Uint8Array.from(bytes);
    malformed[4] = length >> 8;
    malformed[5] = length & 255;
    assert.throws(() => jpegDimensions(malformed), /JPEG/);
  }
  const outsideScan = Uint8Array.from(bytes);
  outsideScan[2] = 0x12;
  assert.throws(() => jpegDimensions(outsideScan), /JPEG/);
  assert.throws(
    () => jpegDimensions(Uint8Array.from([0xff, 0xd8, 0xff, 0, 0xff, 0xd9])),
    /JPEG/,
  );
  assert.throws(
    () =>
      jpegDimensions(
        Uint8Array.from([0xff, 0xd8, 0xff, 2, 0, 2, ...bytes.subarray(2)]),
      ),
    /JPEG/,
  );
});

test("APP metadata is skipped by declared length instead of interpreting embedded fake SOF bytes", () => {
  const bytes = jpeg(160, 100);
  const fake = jpeg(16384, 16384).subarray(2, 15);
  const app = Uint8Array.from([0xff, 0xe1, 0, fake.length + 2, ...fake]);
  const wrapped = Uint8Array.from([
    ...bytes.subarray(0, 2),
    ...app,
    ...bytes.subarray(2),
  ]);
  assert.deepEqual(jpegDimensions(wrapped), { width: 160, height: 100 });
});

test("progressive scans remain supported, while later frame headers and DNL overrides are rejected", () => {
  const bytes = jpeg(160, 100, 0xc2);
  const anotherScan = [0xff, 0xda, 0, 8, 1, 1, 0, 1, 63, 0, 0x12];
  assert.deepEqual(
    jpegDimensions(
      Uint8Array.from([...bytes.subarray(0, -2), ...anotherScan, 0xff, 0xd9]),
    ),
    { width: 160, height: 100 },
  );
  const otherFrame = jpeg(16384, 16384).subarray(2, 15);
  assert.throws(
    () =>
      jpegDimensions(
        Uint8Array.from([...bytes.subarray(0, -2), ...otherFrame, 0xff, 0xd9]),
      ),
    /JPEG/,
  );
  assert.throws(
    () =>
      jpegDimensions(
        Uint8Array.from([
          ...bytes.subarray(0, -2),
          0xff,
          0xdc,
          0,
          4,
          0x40,
          0,
          0xff,
          0xd9,
        ]),
      ),
    /JPEG/,
  );
});
