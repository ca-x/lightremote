import { Link, type MuxStream } from "./link";
import { Records, jsonFrame, parseJSON } from "./records";
export type Dict = Record<string, unknown>;
export function object(value: unknown): Dict {
  if (!value || typeof value !== "object" || Array.isArray(value))
    throw new Error("Invalid host response");
  return value as Dict;
}
export class Control {
  private next = 1;
  private pending = new Map<
    number,
    {
      resolve(value: unknown): void;
      reject(error: Error): void;
      timer: ReturnType<typeof setTimeout>;
    }
  >();
  private writes = Promise.resolve();
  private closed = false;
  constructor(
    readonly stream: MuxStream,
    private onEvent: (event: string, data: unknown) => void,
    private onError: (error: Error) => void,
  ) {
    const records = new Records(1024 * 1024);
    void stream
      .process((bytes) =>
        records.feed(bytes, (bytes) => {
          const message = object(parseJSON(bytes));
          if (typeof message.event === "string") {
            this.onEvent(message.event, message.data);
            return;
          }
          const pending = this.pending.get(Number(message.id));
          if (!pending) return;
          this.pending.delete(Number(message.id));
          clearTimeout(pending.timer);
          if (message.error) {
            const error = object(message.error);
            pending.reject(
              new Error(
                `${String(error.code || "Host error")}: ${String(error.message || "Request failed")}`,
              ),
            );
          } else if ("result" in message) pending.resolve(message.result);
          else pending.reject(new Error("Invalid control reply"));
        }),
      )
      .then(async () => {
        records.finish();
        if (stream.draining) {
          this.rejectPending(
            new Error("Control stream closed while session drains"),
          );
          await stream.end();
          return;
        }
        throw new Error("Host closed control stream");
      })
      .catch((error) => {
        if (!this.closed) {
          this.close(error);
          this.onError(error);
        }
      });
  }
  call(method: string, params: unknown = {}): Promise<unknown> {
    if (this.closed) return Promise.reject(new Error("Control stream closed"));
    if (this.pending.size >= 32)
      return Promise.reject(new Error("Too many outstanding requests"));
    const id = this.next++;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(new Error(`${method} timed out`));
      }, 15_000);
      this.pending.set(id, { resolve, reject, timer });
      const bytes = jsonFrame({ id, method, params });
      if (bytes.length > 1024 * 1024 + 4) {
        clearTimeout(timer);
        this.pending.delete(id);
        reject(new Error("Control request too large"));
        return;
      }
      this.writes = this.writes
        .then(async () => {
          // Large control records must make progress within the stream queue bound.
          for (let offset = 0; offset < bytes.length; offset += 65536)
            await this.stream.write(bytes.subarray(offset, offset + 65536));
        })
        .catch((error) => {
          const pending = this.pending.get(id);
          if (pending) {
            clearTimeout(timer);
            this.pending.delete(id);
            pending.reject(error);
          }
        });
    });
  }
  private rejectPending(error: Error): void {
    this.closed = true;
    for (const pending of this.pending.values()) {
      clearTimeout(pending.timer);
      pending.reject(error);
    }
    this.pending.clear();
  }
  close(error = new Error("Control stream closed")): void {
    this.rejectPending(error);
    this.stream.reset("Control closed");
  }
}
export async function upload(
  link: Link,
  path: string,
  file: File,
  progress: (n: number) => void,
): Promise<unknown> {
  const stream = await link.open("file.put", { path, size: file.size });
  const records = new Records(1024 * 1024);
  let receipt: unknown,
    count = 0;
  // Read the receipt concurrently with writes to avoid duplex deadlocks.
  const incoming = stream
    .process((bytes) =>
      records.feed(bytes, (bytes) => {
        if (++count > 1) throw new Error("Multiple upload receipts");
        receipt = parseJSON(bytes);
      }),
    )
    .then(() => {
      records.finish();
      if (count !== 1) throw new Error("Missing upload receipt");
    });
  void incoming.catch(() => {});
  try {
    let sent = 0;
    for (let offset = 0; offset < file.size; offset += 65536) {
      await stream.write(
        new Uint8Array(await file.slice(offset, offset + 65536).arrayBuffer()),
      );
      sent = Math.min(file.size, offset + 65536);
      progress(sent);
    }
    await stream.end();
    await incoming;
    const result = object(receipt);
    if (result.error) throw new Error("Host rejected upload");
    if (
      Number(result.size) !== file.size ||
      typeof result.sha256 !== "string" ||
      !/^[a-f0-9]{64}$/i.test(result.sha256)
    )
      throw new Error("Invalid upload receipt");
    return result;
  } catch (error) {
    stream.reset("Upload failed");
    throw error;
  }
}
/** Browser fallback is explicitly capped. A streaming save target can be supplied by the UI. */
export async function download(
  link: Link,
  path: string,
  progress: (n: number) => void,
  sink?: {
    write(bytes: Uint8Array): Promise<void>;
    close(): Promise<void>;
    abort(): Promise<void>;
  },
): Promise<Blob | undefined> {
  let stream: MuxStream | undefined;
  try {
    stream = await link.open("file.get", { path });
    await stream.end();
    let received = 0;
    const chunks: Uint8Array<ArrayBuffer>[] = [];
    await stream.process(async (bytes) => {
      received += bytes.length;
      if (sink) await sink.write(bytes);
      else {
        if (received > 64 * 1024 * 1024)
          throw new Error(
            "Download exceeds the 64 MiB browser memory limit. Use a browser with streaming file saves.",
          );
        chunks.push(Uint8Array.from(bytes));
      }
      progress(received);
    });
    if (sink) await sink.close();
    else return new Blob(chunks);
  } catch (error) {
    stream?.reset("Download failed");
    if (sink) await sink.abort().catch(() => {});
    throw error;
  }
}
