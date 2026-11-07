import { Link, type MuxStream } from "./link";
import { Records, jsonFrame, parseJSON } from "./records";
import {
  acceptPermissions,
  parsePermissions,
  type PermissionSnapshot,
  type Scope,
} from "./permissions";

const controlScopes: Record<string, readonly Scope[]> = {
  "clipboard.get": ["clipboard_read"],
  "clipboard.set": ["clipboard_write"],
  "fs.list": ["file_read"],
  "fs.mkdir": ["file_write"],
  "fs.remove": ["file_write"],
  "desktop.displays": ["desktop", "input"],
  "shell.resize": ["shell"],
};
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
    }
  >();
  private writes = Promise.resolve();
  private closed = false;
  private queuedWrites = 0;
  private permissions?: PermissionSnapshot;
  private scopeEpochs = new Map<Scope, AbortController>();
  /** Legacy callers remain compatible until they supply a permission snapshot. */
  setPermissions(value: unknown): void {
    const next = parsePermissions(value);
    if (!acceptPermissions(this.permissions, next)) return;
    this.permissions = next;
    for (const [scope, epoch] of this.scopeEpochs) {
      if (!next.granted.includes(scope)) {
        this.scopeEpochs.delete(scope);
        epoch.abort();
      }
    }
  }
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
            if (message.event === "permissions.changed")
              this.setPermissions(message.data);
            this.onEvent(message.event, message.data);
            return;
          }
          const pending = this.pending.get(Number(message.id));
          if (!pending) return;
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
  call(
    method: string,
    params: unknown = {},
    options: { signal?: AbortSignal } = {},
  ): Promise<unknown> {
    if (this.closed) return Promise.reject(new Error("Control stream closed"));
    // Cancelled entries still occupy the serialized writer until it advances.
    if (this.pending.size >= 32 || this.queuedWrites >= 32)
      return Promise.reject(new Error("Too many outstanding requests"));
    const id = this.next++;
    const signals = new Set<AbortSignal>();
    if (options.signal) signals.add(options.signal);
    const candidates = Object.hasOwn(controlScopes, method)
      ? controlScopes[method]
      : undefined;
    if (candidates) {
      const scope = candidates.find(
        (scope) =>
          !this.permissions || this.permissions.granted.includes(scope),
      );
      if (!scope)
        return Promise.reject(
          new Error("Host permission not granted for control request"),
        );
      let epoch = this.scopeEpochs.get(scope);
      if (!epoch) {
        epoch = new AbortController();
        this.scopeEpochs.set(scope, epoch);
      }
      signals.add(epoch.signal);
    }
    const check = () => {
      if ([...signals].some((signal) => signal.aborted))
        throw new Error(
          "Control request cancelled: permission revoked or caller aborted. Already-started operations may have run and will not be retried.",
        );
    };
    let bytes: Uint8Array;
    try {
      check();
      bytes = jsonFrame({ id, method, params });
      if (bytes.length > 1024 * 1024 + 4)
        throw new Error("Control request too large");
    } catch (error) {
      return Promise.reject(error);
    }
    this.queuedWrites++;
    const response = new Promise<unknown>((resolve, reject) => {
      const cleanup = () => {
        this.pending.delete(id);
        clearTimeout(timer);
        for (const signal of signals)
          signal.removeEventListener("abort", abort);
      };
      const pending = {
        resolve(value: unknown) {
          cleanup();
          resolve(value);
        },
        reject(error: Error) {
          cleanup();
          reject(error);
        },
      };
      const abort = () => {
        try {
          check();
        } catch (error) {
          pending.reject(error as Error);
        }
      };
      const timer = setTimeout(
        () => pending.reject(new Error(`${method} timed out`)),
        15_000,
      );
      this.pending.set(id, pending);
      for (const signal of signals)
        signal.addEventListener("abort", abort, { once: true });
      this.writes = this.writes
        .then(async () => {
          if (!this.pending.has(id) || this.closed) return;
          check();
          // Once the first chunk starts, finish the entire record even if revoked.
          // Dropping a suffix would corrupt framing for unrelated queued requests.
          // The caller is rejected immediately and late replies are discarded.
          try {
            for (let offset = 0; offset < bytes.length; offset += 65536)
              await this.stream.write(bytes.subarray(offset, offset + 65536));
          } catch (error) {
            // A failed write may leave a partial frame. Never append a sibling.
            if (!this.closed) {
              const failure =
                error instanceof Error
                  ? error
                  : new Error("Control write failed");
              this.close(failure);
              this.onError(failure);
            }
          }
        })
        .catch((error) => this.pending.get(id)?.reject(error))
        .finally(() => {
          this.queuedWrites--;
        });
    });
    // Also cover a revoke notification coalesced after the reply, before its
    // promise continuation runs. A later grant never revives this epoch.
    return response.then((value) => {
      check();
      return value;
    });
  }
  private rejectPending(error: Error): void {
    this.closed = true;
    for (const pending of this.pending.values()) {
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
  signal?: AbortSignal,
): Promise<unknown> {
  signal?.throwIfAborted();
  const stream = await link.open("file.put", { path, size: file.size });
  const abort = () => stream.reset("Upload cancelled");
  signal?.addEventListener("abort", abort, { once: true });
  if (signal?.aborted) abort();
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
    signal?.throwIfAborted();
    let sent = 0;
    for (let offset = 0; offset < file.size; offset += 65536) {
      const bytes = new Uint8Array(
        await file.slice(offset, offset + 65536).arrayBuffer(),
      );
      signal?.throwIfAborted();
      await stream.write(bytes);
      sent = Math.min(file.size, offset + 65536);
      progress(sent);
    }
    await stream.end();
    await incoming;
    signal?.throwIfAborted();
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
  } finally {
    signal?.removeEventListener("abort", abort);
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
  signal?: AbortSignal,
): Promise<Blob | undefined> {
  let stream: MuxStream | undefined;
  const abort = () => stream?.reset("Download cancelled");
  signal?.addEventListener("abort", abort, { once: true });
  try {
    signal?.throwIfAborted();
    stream = await link.open("file.get", { path });
    if (signal?.aborted) abort();
    signal?.throwIfAborted();
    await stream.end();
    let received = 0;
    const chunks: Uint8Array<ArrayBuffer>[] = [];
    await stream.process(async (bytes) => {
      signal?.throwIfAborted();
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
    signal?.throwIfAborted();
    if (sink) await sink.close();
    else return new Blob(chunks);
  } catch (error) {
    stream?.reset("Download failed");
    if (sink) await sink.abort().catch(() => {});
    throw error;
  } finally {
    signal?.removeEventListener("abort", abort);
  }
}
