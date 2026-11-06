import { test } from "node:test";
import assert from "node:assert/strict";
import { Control } from "../src/client.ts";
import { Link } from "../src/link.ts";
import { HostFixture } from "./host-fixture.ts";

function barrier() {
  let release!: () => void;
  const promise = new Promise<void>((resolve) => {
    release = resolve;
  });
  return { promise, release };
}
async function until(check: () => boolean) {
  const deadline = Date.now() + 3000;
  while (!check()) {
    if (Date.now() > deadline) throw new Error("Condition timeout");
    await new Promise((resolve) => setTimeout(resolve, 5));
  }
}

test("GOAWAY drains an accepted download, refuses new opens, then waits for end-to-end Pong before carrier close", async () => {
  const fixture = await HostFixture.start();
  fixture.holdPongs = true;
  let link: Link | undefined;
  let closed = false;
  try {
    const close = barrier();
    link = await Link.connect(fixture.invite(), () => {
      closed = true;
      close.release();
    });
    const stream = await link.open("file.get", { path: "large.bin" });
    await stream.end();
    const entered = barrier(),
      resume = barrier();
    const chunks: Uint8Array[] = [];
    const incoming = stream.process(async (bytes) => {
      chunks.push(bytes);
      if (chunks.length === 1) {
        entered.release();
        await resume.promise;
      }
    });
    void incoming.catch(() => {});
    await entered.promise;
    fixture.goaway();
    await until(() => link!.draining || closed);
    assert.equal(closed, false, "GOAWAY must preserve accepted streams");
    await assert.rejects(
      link.open("shell", { cols: 80, rows: 24 }),
      /draining/,
    );
    assert.equal(
      fixture.pings,
      0,
      "No drain probe before active download is consumed",
    );
    resume.release();
    await incoming;
    assert.deepEqual(
      Buffer.concat(chunks),
      Buffer.from(fixture.files.get("large.bin")!),
    );
    await until(() => fixture.pings === 1);
    assert.equal(
      fixture.connectionCreditReturned,
      700000,
      "Final WINDOW credit precedes the probe",
    );
    assert.equal(closed, false, "Carrier stays open while Pong is withheld");
    fixture.releasePongs();
    await close.promise;
    assert.deepEqual(fixture.errors, []);
  } finally {
    link?.close();
    await fixture.close();
  }
});

test("GOAWAY control FIN half-closes control without aborting another accepted download", async () => {
  const fixture = await HostFixture.start();
  let link: Link | undefined;
  const resume = barrier();
  try {
    let closed = false;
    const close = barrier();
    link = await Link.connect(fixture.invite(), () => {
      closed = true;
      close.release();
    });
    const controlStream = await link.open("control");
    const errors: Error[] = [];
    const control = new Control(
      controlStream,
      () => {},
      (error) => {
        errors.push(error);
        link!.close(error);
      },
    );
    await control.call("capabilities");
    const stream = await link.open("file.get", { path: "large.bin" });
    await stream.end();
    const entered = barrier();
    const chunks: Uint8Array[] = [];
    const incoming = stream.process(async (bytes) => {
      chunks.push(bytes);
      if (chunks.length === 1) {
        entered.release();
        await resume.promise;
      }
    });
    void incoming.catch(() => {});
    await entered.promise;
    fixture.goaway();
    await until(() => link!.draining);
    fixture.endControl();
    await until(() => controlStream.localFin || errors.length > 0);
    assert.deepEqual(
      errors,
      [],
      "Normal control FIN during GOAWAY is not a session failure",
    );
    assert.equal(
      closed,
      false,
      "Accepted download remains active after control half-close",
    );
    await assert.rejects(control.call("capabilities"), /closed/);
    resume.release();
    await incoming;
    assert.deepEqual(
      Buffer.concat(chunks),
      Buffer.from(fixture.files.get("large.bin")!),
    );
    await close.promise;
    assert.deepEqual(fixture.errors, []);
  } finally {
    resume.release();
    link?.close();
    await fixture.close();
  }
});

test("GOAWAY preserves duplex shell input/output until the accepted shell is closed", async () => {
  const fixture = await HostFixture.start();
  let link: Link | undefined;
  try {
    const close = barrier();
    let closed = false;
    link = await Link.connect(fixture.invite(), () => {
      closed = true;
      close.release();
    });
    const stream = await link.open("shell", { cols: 80, rows: 24 });
    (await stream.read())!.consume();
    fixture.goaway();
    await until(() => link!.draining);
    const input = new TextEncoder().encode("after GOAWAY\r");
    await stream.write(input);
    const echoed = await stream.read();
    assert.deepEqual(echoed!.bytes, input);
    echoed!.consume();
    assert.equal(closed, false);
    stream.reset("Accepted shell closed");
    await close.promise;
    assert.ok(fixture.pings > 0);
    assert.deepEqual(fixture.errors, []);
  } finally {
    link?.close();
    await fixture.close();
  }
});
