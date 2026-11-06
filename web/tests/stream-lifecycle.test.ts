import { test } from "node:test";
import assert from "node:assert/strict";
import { Link, MuxStream } from "../src/link.ts";
import { download } from "../src/client.ts";
import { HostFixture } from "./host-fixture.ts";

function barrier() {
  let release!: () => void;
  const promise = new Promise<void>((resolve) => {
    release = resolve;
  });
  return { promise, release };
}

for (const path of ["hello.txt", "empty.bin"]) {
  for (const failure of ["RESET", "carrier loss"] as const) {
    test(`final FIN processing rejects ${failure} during its async handler (${path})`, async () => {
      const fixture = await HostFixture.start();
      fixture.files.set("empty.bin", new Uint8Array());
      let link: Link | undefined;
      try {
        link = await Link.connect(fixture.invite());
        const stream = await link.open("file.get", { path });
        await stream.end();
        const entered = barrier(),
          processing = barrier();
        const operation = stream.process(async (bytes) => {
          assert.equal(bytes.length === 0, path === "empty.bin");
          assert.equal(stream.remoteFin, true);
          entered.release();
          await processing.promise;
        });
        const rejected = assert.rejects(operation, /reviewer race/);
        await entered.promise;
        const windows = fixture.windows;
        if (failure === "RESET") stream.reset("reviewer race RESET");
        else link.close(new Error("reviewer race carrier loss"));
        processing.release();
        await rejected;
        assert.equal(
          fixture.windows,
          windows,
          "Failed processing returns no receive credit",
        );
      } finally {
        link?.close();
        await fixture.close();
      }
    });

    test(`download aborts its sink without close or success after ${failure} during final write (${path})`, async () => {
      const fixture = await HostFixture.start();
      fixture.files.set("empty.bin", new Uint8Array());
      let link: Link | undefined;
      try {
        link = await Link.connect(fixture.invite());
        let stream: MuxStream | undefined;
        const open = link.open.bind(link);
        link.open = async (...args) => {
          stream = await open(...args);
          return stream;
        };
        const entered = barrier(),
          processing = barrier();
        const actions: string[] = [];
        let successful = false;
        const operation = download(link, path, () => {}, {
          async write() {
            actions.push("write");
            entered.release();
            await processing.promise;
          },
          async close() {
            actions.push("close");
          },
          async abort() {
            actions.push("abort");
          },
        }).then((result) => {
          successful = true;
          return result;
        });
        const rejected = assert.rejects(operation, /reviewer race/);
        await entered.promise;
        assert.equal(stream!.remoteFin, true);
        if (failure === "RESET") stream!.reset("reviewer race RESET");
        else link.close(new Error("reviewer race carrier loss"));
        processing.release();
        await rejected;
        assert.equal(successful, false);
        assert.deepEqual(actions, ["write", "abort"]);
      } finally {
        link?.close();
        await fixture.close();
      }
    });
  }
}

test("final FIN processing rejects a synchronous failure while returning consume credit", async () => {
  const failure = new Error("credit return failed");
  const owner = {
    retire() {},
    consume(stream: MuxStream) {
      stream.fail(failure);
    },
  } as unknown as Link;
  const stream = new MuxStream(1, owner);
  stream.receive(new Uint8Array([42]), true);
  await assert.rejects(
    stream.process(() => {}),
    (error) => error === failure,
  );
});

test("processing rejects a failure between an already-resolved EOF read and its continuation", async () => {
  const owner = { retire() {}, consume() {} } as unknown as Link;
  const stream = new MuxStream(1, owner);
  stream.receive(new Uint8Array(), true);
  (await stream.read())!.consume();
  const operation = stream.process(() => {});
  stream.fail(new Error("EOF continuation race"));
  await assert.rejects(operation, /EOF continuation race/);
});
