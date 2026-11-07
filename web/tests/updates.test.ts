import { test } from "node:test";
import assert from "node:assert/strict";
import { isNewerVersion, versionOf, watchUpdates } from "../src/updates";
const settle = () => new Promise((resolve) => setImmediate(resolve));
test("refresh uses stable release upgrades only, never main, prereleases or rollbacks", () => {
  assert.equal(isNewerVersion("v0.2.0", "0.1.11"), true);
  assert.equal(isNewerVersion("1.0.0", "0.99.99"), true);
  assert.equal(isNewerVersion("1.0.0+build.2", "1.0.0+build.1"), false);
  assert.equal(isNewerVersion("1.0.1+build.1", "1.0.0"), true);
  for (const version of ["0.1.1", "v0.1.2", "0.1.3-beta.1", "main", "012.0.0"])
    assert.equal(isNewerVersion(version, "0.1.2"), false);
  assert.equal(versionOf({ version: "main" }), undefined);
  assert.equal(versionOf({ version: "v0.2.0" }), "v0.2.0");
});
test("bounded no-store manifest polling announces each deployed version once and stops", async () => {
  let value: unknown = { version: "0.2.0" },
    requests = 0;
  const found: string[] = [];
  const updates = watchUpdates({
    currentVersion: "0.1.1",
    onUpdate: (v) => found.push(v),
    fetch: (async (url, options) => {
      requests++;
      assert.equal(url, "/version.json");
      assert.equal(options?.cache, "no-store");
      assert.ok(options?.signal);
      return new Response(JSON.stringify(value));
    }) as typeof fetch,
  });
  try {
    await settle();
    await updates.check();
    assert.deepEqual(found, ["0.2.0"]);
    value = { version: "0.1.0" };
    await updates.check();
    assert.equal(found.length, 1);
    value = { version: "0.3.0" };
    await updates.check();
    assert.deepEqual(found, ["0.2.0", "0.3.0"]);
    updates.stop();
    const count = requests;
    await updates.check();
    assert.equal(requests, count);
  } finally {
    updates.stop();
  }
});
test("offline, failed or invalid responses never report new versions; unknown build establishes baseline", async () => {
  let online = false,
    value = { version: "0.2.0" },
    requests = 0;
  const found: string[] = [];
  const updates = watchUpdates({
    online: () => online,
    onUpdate: (v) => found.push(v),
    fetch: (async () => {
      requests++;
      return new Response(JSON.stringify(value));
    }) as typeof fetch,
  });
  try {
    await settle();
    assert.equal(requests, 0);
    online = true;
    await updates.check();
    assert.deepEqual(found, []);
    value = { version: "main" };
    await updates.check();
    assert.deepEqual(found, []);
    value = { version: "0.3.0" };
    await updates.check();
    assert.deepEqual(found, ["0.3.0"]);
  } finally {
    updates.stop();
  }
});
test("cleanup aborts a pending manifest; timeout failures are quiet", async () => {
  let signal: AbortSignal | undefined;
  const found: string[] = [];
  const updates = watchUpdates({
    currentVersion: "0.1.1",
    timeoutMs: 10,
    onUpdate: (v) => found.push(v),
    fetch: ((_url, options) => {
      signal = options?.signal as AbortSignal;
      return new Promise((_yes, no) =>
        signal?.addEventListener("abort", () => no(new Error("aborted")), {
          once: true,
        }),
      );
    }) as typeof fetch,
  });
  assert.ok(signal);
  updates.stop();
  assert.equal(signal.aborted, true);
  await settle();
  assert.equal(found.length, 0);
  const timed = watchUpdates({
    currentVersion: "0.1.1",
    timeoutMs: 5,
    onUpdate: (v) => found.push(v),
    fetch: ((_url, options) =>
      new Promise((_yes, no) =>
        options?.signal?.addEventListener(
          "abort",
          () => no(new Error("timeout")),
          { once: true },
        ),
      )) as typeof fetch,
  });
  try {
    await new Promise((resolve) => setTimeout(resolve, 15));
    assert.deepEqual(found, []);
  } finally {
    timed.stop();
  }
});
