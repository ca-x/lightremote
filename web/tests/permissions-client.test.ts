import { test } from "node:test";
import assert from "node:assert/strict";
import { Link, type MuxStream } from "../src/link";
import { Control, upload, download } from "../src/client";
import { HostFixture } from "./host-fixture";
import { Records, jsonFrame } from "../src/records";
import {
  scopes,
  type Scope,
  type PermissionSnapshot,
} from "../src/permissions";

function barrier() {
  let release!: () => void;
  const promise = new Promise<void>((yes) => {
    release = yes;
  });
  return { promise, release };
}
test("permission cancellation during final download write aborts the sink and never reports success", async () => {
  const fixture = await HostFixture.start();
  const abort = new AbortController();
  let link: Link | undefined;
  try {
    link = await Link.connect(fixture.invite());
    const entered = barrier(),
      done = barrier();
    const actions: string[] = [];
    const operation = download(
      link,
      "hello.txt",
      () => {},
      {
        async write() {
          actions.push("write");
          entered.release();
          await done.promise;
        },
        async close() {
          actions.push("close");
        },
        async abort() {
          actions.push("abort");
        },
      },
      abort.signal,
    );
    const rejected = assert.rejects(operation, /cancelled|aborted/i);
    await entered.promise;
    abort.abort();
    done.release();
    await rejected;
    assert.deepEqual(actions, ["write", "abort"]);
  } finally {
    link?.close();
    await fixture.close();
  }
});
test("already revoked transfers open no stream and abort an acquired save destination", async () => {
  const controller = new AbortController();
  controller.abort();
  let opened = 0,
    aborted = 0;
  const owner = {
    async open() {
      opened++;
      throw new Error("must not open");
    },
  } as unknown as Link;
  await assert.rejects(
    download(
      owner,
      "hello.txt",
      () => {},
      {
        async write() {},
        async close() {},
        async abort() {
          aborted++;
        },
      },
      controller.signal,
    ),
    /aborted/i,
  );
  await assert.rejects(
    upload(
      owner,
      "a.txt",
      new File(["hello"], "a.txt"),
      () => {},
      controller.signal,
    ),
    /aborted/i,
  );
  assert.equal(opened, 0);
  assert.equal(aborted, 1);
});
test("revocation while an upload waits for credit resets its stream without committing", async () => {
  const fixture = await HostFixture.start();
  fixture.withholdCredit = true;
  const controller = new AbortController(),
    entered = barrier();
  let link: Link | undefined;
  try {
    link = await Link.connect(fixture.invite());
    const open = link.open.bind(link);
    link.open = async (...args) => {
      const stream = await open(...args);
      const write = stream.write.bind(stream);
      stream.write = (bytes) => {
        entered.release();
        return write(bytes);
      };
      return stream;
    };
    const operation = upload(
      link,
      "cancelled.bin",
      new File([new Uint8Array(200000)], "cancelled.bin"),
      () => {},
      controller.signal,
    );
    const rejected = assert.rejects(operation, /cancelled|aborted/i);
    await entered.promise;
    controller.abort();
    await rejected;
    assert.equal(fixture.files.has("cancelled.bin"), false);
  } finally {
    link?.close();
    await fixture.close();
  }
});

test("a side-effect request that expires behind a stalled write is never sent later", async (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const writing = barrier(),
    release = barrier();
  const writes: Uint8Array[] = [];
  const stream = {
    async process() {
      await new Promise<void>(() => {});
    },
    async write(bytes: Uint8Array) {
      writes.push(bytes);
      writing.release();
      await release.promise;
    },
    reset() {},
  } as unknown as MuxStream;
  const control = new Control(
    stream,
    () => {},
    () => {},
  );
  const first = assert.rejects(
    control.call("fs.mkdir", { path: "first" }),
    /timed out/,
  );
  const second = assert.rejects(
    control.call("fs.mkdir", { path: "second" }),
    /timed out/,
  );
  await writing.promise;
  t.mock.timers.tick(15_000);
  await Promise.all([first, second]);
  release.release();
  await new Promise((resolve) => setImmediate(resolve));
  assert.equal(writes.length, 1);
  control.close();
});

// Outbound backpressure must not block independent host notifications.
function queuedControl() {
  const entered = barrier(),
    release = barrier();
  const writes: Uint8Array[] = [];
  const errors: Error[] = [];
  let resets = 0;
  let receive!: (bytes: Uint8Array) => void | Promise<void>;
  const stream = {
    async process(handle: (bytes: Uint8Array) => void | Promise<void>) {
      receive = handle;
      await new Promise<void>(() => {});
    },
    async write(bytes: Uint8Array) {
      writes.push(Uint8Array.from(bytes));
      if (writes.length === 1) {
        entered.release();
        await release.promise;
      }
    },
    reset() {
      resets++;
    },
  } as unknown as MuxStream;
  const control = new Control(
    stream,
    () => {},
    (error) => errors.push(error),
  );
  return {
    control,
    entered,
    release,
    writes,
    errors,
    get resets() {
      return resets;
    },
    async host(message: unknown) {
      await receive(jsonFrame(message));
    },
    requests() {
      const records = new Records(1024 * 1024),
        requests: { id: number; method: string }[] = [];
      for (const bytes of writes)
        records.feed(bytes, (bytes) => {
          requests.push(JSON.parse(new TextDecoder().decode(bytes)));
        });
      records.finish();
      return requests;
    },
  };
}
function grants(revision: number, granted: Scope[]): PermissionSnapshot {
  return {
    available: [...scopes],
    granted,
    pending: null,
    last_decision: null,
    revision,
    mode: "attended",
  };
}
for (const [method, scope] of [
  ["clipboard.set", "clipboard_write"],
  ["clipboard.get", "clipboard_read"],
  ["fs.mkdir", "file_write"],
  ["fs.remove", "file_write"],
  ["fs.list", "file_read"],
  ["desktop.displays", "desktop"],
  ["shell.resize", "shell"],
] as const) {
  test(`queued ${method} is permanently cancelled by revoke/regrant without sending or success`, async () => {
    const fixture = queuedControl();
    const { control } = fixture;
    try {
      control.setPermissions(grants(1, [...scopes]));
      const blocker = control.call("fs.list");
      await fixture.entered.promise;
      let successful = false;
      const operation = control.call(method, {}).then(() => {
        successful = true;
      });
      const rejected = assert.rejects(operation, /revoked|cancelled/i);
      const blockerResult =
        scope === "file_read"
          ? assert.rejects(blocker, /revoked|cancelled/i)
          : blocker;
      await fixture.host({
        event: "permissions.changed",
        data: grants(
          2,
          scopes.filter((s) => s !== scope),
        ),
      });
      await fixture.host({
        event: "permissions.changed",
        data: grants(3, [...scopes]),
      });
      await rejected;
      fixture.release.release();
      await new Promise((resolve) => setImmediate(resolve));
      assert.deepEqual(
        fixture.requests().map((r) => r.method),
        ["fs.list"],
      );
      await fixture.host({ id: 2, result: {} });
      await fixture.host({ id: 1, result: {} });
      await blockerResult;
      assert.equal(successful, false);
      const fresh = control.call(method, {});
      await new Promise((resolve) => setImmediate(resolve));
      const last = fixture.requests().at(-1)!;
      assert.equal(last.method, method);
      await fixture.host({ id: last.id, result: { fresh: true } });
      assert.deepEqual(await fresh, { fresh: true });
    } finally {
      fixture.release.release();
      control.close();
    }
  });
}

test("revoking a partially sent record finishes its frame but rejects late success", async () => {
  const fixture = queuedControl();
  const { control } = fixture;
  try {
    control.setPermissions(grants(1, [...scopes]));
    let successful = false;
    const call = control
      .call("clipboard.set", { text: "x".repeat(140000) })
      .then(() => {
        successful = true;
      });
    const rejected = assert.rejects(call, /revoked|cancelled/i);
    await fixture.entered.promise;
    await fixture.host({
      event: "permissions.changed",
      data: grants(
        2,
        scopes.filter((s) => s !== "clipboard_write"),
      ),
    });
    await fixture.host({
      event: "permissions.changed",
      data: grants(3, [...scopes]),
    });
    await rejected;
    const next = control.call("permissions.get");
    fixture.release.release();
    await new Promise((resolve) => setImmediate(resolve));
    assert.ok(fixture.writes.length >= 4);
    assert.deepEqual(
      fixture.requests().map((r) => r.method),
      ["clipboard.set", "permissions.get"],
    );
    await fixture.host({ id: 1, result: {} });
    await fixture.host({ id: 2, result: { available: true } });
    assert.deepEqual(await next, { available: true });
    assert.equal(successful, false);
  } finally {
    fixture.release.release();
    control.close();
  }
});

test("an explicit AbortSignal cancels only its queued call and cleans its listener", async (t) => {
  const fixture = queuedControl(),
    abort = new AbortController();
  const remove = t.mock.method(abort.signal, "removeEventListener");
  try {
    const first = fixture.control.call("capabilities");
    await fixture.entered.promise;
    const cancelled = assert.rejects(
      fixture.control.call("fs.mkdir", {}, { signal: abort.signal }),
      /cancelled/i,
    );
    const sibling = fixture.control.call("permissions.get");
    abort.abort();
    await cancelled;
    assert.equal(remove.mock.callCount(), 1);
    fixture.release.release();
    await new Promise((resolve) => setImmediate(resolve));
    assert.deepEqual(
      fixture.requests().map((r) => r.method),
      ["capabilities", "permissions.get"],
    );
    await fixture.host({ id: 1, result: {} });
    await fixture.host({ id: 3, result: {} });
    await Promise.all([first, sibling]);
  } finally {
    fixture.release.release();
    fixture.control.close();
  }
});

test("revocation after a reply but before continuation cannot report success", async () => {
  const fixture = queuedControl();
  try {
    fixture.control.setPermissions(grants(1, [...scopes]));
    const operation = fixture.control.call("clipboard.set", {});
    const rejected = assert.rejects(operation, /revoked|cancelled/i);
    await fixture.entered.promise;
    // Coalesced host records arrive in one reader turn, before promise handlers.
    const response = fixture.host({ id: 1, result: {} });
    const revoke = fixture.host({
      event: "permissions.changed",
      data: grants(2, []),
    });
    const regrant = fixture.host({
      event: "permissions.changed",
      data: grants(3, [...scopes]),
    });
    await Promise.all([response, revoke, regrant, rejected]);
  } finally {
    fixture.release.release();
    fixture.control.close();
  }
});

test("aborted queued entries remain bounded and timeout/close/success remove listeners", async (t) => {
  t.mock.timers.enable({ apis: ["setTimeout"] });
  const fixture = queuedControl();
  try {
    const first = assert.rejects(
      fixture.control.call("capabilities"),
      /timed out/,
    );
    await fixture.entered.promise;
    for (let i = 0; i < 31; i++) {
      const abort = new AbortController();
      const cancelled = assert.rejects(
        fixture.control.call("fs.mkdir", {}, { signal: abort.signal }),
        /cancelled/i,
      );
      abort.abort();
      await cancelled;
    }
    await assert.rejects(fixture.control.call("permissions.get"), /Too many/);
    t.mock.timers.tick(15_000);
    await first;
    fixture.release.release();
    await new Promise((resolve) => setImmediate(resolve));
    assert.deepEqual(
      fixture.requests().map((r) => r.method),
      ["capabilities"],
    );
    for (const outcome of ["timeout", "success", "close"] as const) {
      const abort = new AbortController(),
        remove = t.mock.method(abort.signal, "removeEventListener");
      const operation = fixture.control.call(
        "permissions.get",
        {},
        { signal: abort.signal },
      );
      await new Promise((resolve) => setImmediate(resolve));
      if (outcome === "success") {
        await fixture.host({ id: fixture.requests().at(-1)!.id, result: {} });
        await operation;
      } else {
        const rejected = assert.rejects(
          operation,
          outcome === "timeout" ? /timed out/ : /closed/,
        );
        if (outcome === "timeout") t.mock.timers.tick(15_000);
        else fixture.control.close();
        await rejected;
      }
      assert.equal(remove.mock.callCount(), 1);
    }
  } finally {
    fixture.release.release();
    fixture.control.close();
  }
});

test("displays may use input permission; unrelated revisions and permission RPCs stay usable", async () => {
  const fixture = queuedControl();
  try {
    fixture.control.setPermissions(grants(1, ["input"]));
    const displays = fixture.control.call("desktop.displays");
    await fixture.entered.promise;
    await assert.rejects(fixture.control.call("clipboard.set"), /not granted/);
    const rpc = fixture.control.call("permissions.request", {
      scopes: ["desktop"],
    });
    await fixture.host({
      event: "permissions.changed",
      data: grants(2, ["input", "clipboard_read"]),
    });
    fixture.release.release();
    await new Promise((resolve) => setImmediate(resolve));
    assert.deepEqual(
      fixture.requests().map((r) => r.method),
      ["desktop.displays", "permissions.request"],
    );
    for (const request of fixture.requests())
      await fixture.host({ id: request.id, result: {} });
    await Promise.all([displays, rpc]);
  } finally {
    fixture.release.release();
    fixture.control.close();
  }
});

test("a partial control write failure resets the stream and never sends queued siblings", async () => {
  const fixture = queuedControl();
  const write = fixture.control.stream.write.bind(fixture.control.stream);
  let attempts = 0;
  fixture.control.stream.write = async (bytes) => {
    if (++attempts === 2) throw new Error("injected partial write failure");
    return write(bytes);
  };
  const first = assert.rejects(
    fixture.control.call("clipboard.set", { text: "x".repeat(140000) }),
    /partial write failure/,
  );
  await fixture.entered.promise;
  const sibling = assert.rejects(
    fixture.control.call("permissions.get"),
    /partial write failure/,
  );
  fixture.release.release();
  // Bound the assertion without waiting for a real request timeout on the old implementation.
  await new Promise((resolve) => setImmediate(resolve));
  try {
    assert.equal(fixture.resets, 1);
    assert.equal(attempts, 2, "No sibling may follow a truncated record");
    assert.equal(fixture.writes.length, 1);
    assert.equal(fixture.errors.length, 1);
    await Promise.all([first, sibling]);
    await assert.rejects(fixture.control.call("capabilities"), /closed/);
  } finally {
    fixture.control.close(new Error("injected partial write failure"));
    await Promise.all([first, sibling]);
  }
});
