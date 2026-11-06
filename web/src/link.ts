import {
  Session,
  Class,
  ErrorCode,
  type Event,
} from "../node_modules/@lib-x/tmp/dist/session.js";
import {
  encode,
  IncrementalDecoder,
} from "../node_modules/@lib-x/tmp/dist/framing.js";
import type { Packet } from "@lib-x/tmp/model";
import {
  Cipher,
  makeHello,
  parseHello,
  unbase64url,
  websocketURL,
  MAX_WIRE,
  type Invite,
} from "./secure";

export const PROFILES = ["control", "desktop", "input", "shell", "file"].map(
  (name) => `lightremote.${name}.v1`,
);
const STREAM_WINDOW = 128 * 1024,
  CONNECTION_WINDOW = 1024 * 1024;
const MAX_QUEUE = 2 * 1024 * 1024,
  MAX_CHUNKS = 256;
function deferred<T>() {
  let resolve!: (value: T) => void, reject!: (error: Error) => void;
  const promise = new Promise<T>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}
function errorOf(value: unknown): Error {
  return value instanceof Error ? value : new Error(String(value));
}
interface Write {
  bytes: Uint8Array;
  offset: number;
  fin: boolean;
  done: ReturnType<typeof deferred<void>>;
}
export interface Chunk {
  bytes: Uint8Array;
  fin: boolean;
  consume(): void;
}
interface BufferedChunk extends Chunk {
  storage: Uint8Array;
}
/** Each read requires explicit consume, after processing or bounded acceptance. */
export class MuxStream {
  readonly accepted = deferred<void>();
  readonly writes: Write[] = [];
  queuedSend = 0;
  localFin = false;
  remoteFin = false;
  remoteFinConsumed = false;
  outstanding = 0;
  acked = false;
  private ending = false;
  private queue: BufferedChunk[] = [];
  private waiter?: ReturnType<typeof deferred<Chunk | null>>;
  private error?: Error;
  constructor(
    readonly id: number,
    private link: Link,
  ) {
    // Rejections are still observable by open(); avoid an unhandled rejection on early carrier failure.
    void this.accepted.promise.catch(() => {});
  }
  get draining(): boolean {
    return this.link.draining;
  }
  write(bytes: Uint8Array): Promise<void> {
    return this.send(bytes, false);
  }
  end(): Promise<void> {
    return this.send(new Uint8Array(), true);
  }
  private send(bytes: Uint8Array, fin: boolean): Promise<void> {
    if (this.error) return Promise.reject(this.error);
    if (this.ending)
      return Promise.reject(new Error("Stream send half is closed"));
    if (!fin && !bytes.length) return Promise.resolve();
    if (
      this.queuedSend + bytes.length > STREAM_WINDOW ||
      this.writes.length >= MAX_CHUNKS
    )
      return Promise.reject(new Error("Stream send queue is full"));
    this.ending = fin;
    const done = deferred<void>();
    this.writes.push({ bytes: Uint8Array.from(bytes), offset: 0, fin, done });
    this.queuedSend += bytes.length;
    this.link.pump();
    return done.promise;
  }
  read(): Promise<Chunk | null> {
    if (this.error) return Promise.reject(this.error);
    const chunk = this.queue.shift();
    if (chunk) return Promise.resolve(chunk);
    if (this.remoteFin) return Promise.resolve(null);
    if (this.waiter)
      return Promise.reject(new Error("Concurrent stream reads"));
    this.waiter = deferred<Chunk | null>();
    return this.waiter.promise;
  }
  async process(
    handle: (bytes: Uint8Array) => void | Promise<void>,
  ): Promise<void> {
    for (;;) {
      const chunk = await this.read();
      this.throwIfFailed();
      if (!chunk) return;
      await handle(chunk.bytes);
      // RESET/carrier failure can arrive while application processing awaits.
      this.throwIfFailed();
      chunk.consume();
      // Returning credit can synchronously fail the link as well.
      this.throwIfFailed();
      if (chunk.fin) return;
    }
  }
  private throwIfFailed(): void {
    if (this.error) throw this.error;
  }
  reset(reason = "Cancelled"): void {
    this.link.reset(this, reason);
  }
  receive(bytes: Uint8Array, fin: boolean): void {
    if (this.error) return;
    if (
      this.remoteFin ||
      this.outstanding + bytes.length > STREAM_WINDOW
    ) {
      this.reset("Receive queue exceeded");
      return;
    }
    this.remoteFin = fin;
    this.outstanding += bytes.length;
    // Only queued, undispatched input may be combined. Its total byte size is
    // already bounded by receive credit, regardless of how many packets arrive.
    const tail = this.queue.at(-1);
    if (tail) {
      const length = tail.bytes.length + bytes.length;
      if (length > tail.storage.length) {
        const storage = new Uint8Array(
          Math.min(STREAM_WINDOW, Math.max(16384, tail.storage.length * 2, length)),
        );
        storage.set(tail.bytes);
        tail.storage = storage;
      }
      tail.storage.set(bytes, tail.bytes.length);
      tail.bytes = tail.storage.subarray(0, length);
      tail.fin = fin;
      this.link.retire(this);
      return;
    }
    let consumed = false;
    const chunk: BufferedChunk = {
      bytes,
      storage: bytes,
      fin,
      consume: () => {
        if (consumed || this.error) return;
        consumed = true;
        if (chunk.fin) this.remoteFinConsumed = true;
        this.outstanding -= chunk.bytes.length;
        this.link.consume(this, chunk.bytes.length);
      },
    };
    if (this.waiter) {
      const waiter = this.waiter;
      this.waiter = undefined;
      waiter.resolve(chunk);
    } else this.queue.push(chunk);
    this.link.retire(this);
  }
  fail(error: Error): void {
    this.error = error;
    this.queue = [];
    this.accepted.reject(error);
    this.waiter?.reject(error);
    this.waiter = undefined;
    for (const write of this.writes.splice(0)) write.done.reject(error);
    this.queuedSend = 0;
  }
}
/** Single synchronous TMP owner; independent bounded async carrier read and write progress. */
export class Link {
  private readonly session = new Session(
    "initiator",
    {
      maxData: 16384,
      streamWindow: STREAM_WINDOW,
      connectionWindow: CONNECTION_WINDOW,
      maxStreams: 16,
    },
    PROFILES,
  );
  private readonly streams = new Map<number, MuxStream>();
  private readonly ready = deferred<void>();
  private readonly decoder = new IncrementalDecoder((packet) =>
    this.receive(packet),
  );
  private outbound: Uint8Array[] = [];
  private outboundSize = 0;
  private sending = false;
  private inboundSize = 0;
  private inboundCount = 0;
  private inbound: Promise<void> = Promise.resolve();
  private tx?: Cipher;
  private rx?: Cipher;
  private dead = false;
  private pumping = false;
  private pumpScheduled = false;
  private handshakeReceived = false;
  private drainReason = "";
  private drainProbe?: bigint;
  private drainAcknowledged = false;
  private drainDeadline?: ReturnType<typeof setTimeout>;
  private drainPoll?: ReturnType<typeof setTimeout>;
  private timeout: ReturnType<typeof setTimeout>;
  private constructor(
    private socket: WebSocket,
    private invite: Invite,
    readonly onClose: (reason: Error) => void,
  ) {
    this.timeout = setTimeout(
      () => this.close(new Error("Encrypted TMP handshake timed out")),
      20_000,
    );
    const hello = makeHello("client");
    socket.binaryType = "arraybuffer";
    socket.onopen = () => {
      if (socket.protocol !== "lightremote.v1") {
        this.close(new Error("Relay did not select lightremote.v1"));
        return;
      }
      socket.send(new TextEncoder().encode(JSON.stringify(hello)));
    };
    socket.onmessage = (event) => {
      if (
        !(event.data instanceof ArrayBuffer) ||
        event.data.byteLength < 1 ||
        event.data.byteLength > MAX_WIRE ||
        this.inboundCount >= 256 ||
        this.inboundSize + event.data.byteLength > MAX_QUEUE
      ) {
        this.close(
          new Error("Invalid carrier record or receive queue overflow"),
        );
        return;
      }
      const bytes = new Uint8Array(event.data);
      this.inboundSize += bytes.length;
      this.inboundCount++;
      this.inbound = this.inbound
        .then(async () => {
          if (this.dead) return;
          if (!this.handshakeReceived) {
            const host = parseHello(bytes, "host");
            this.handshakeReceived = true;
            const secret = unbase64url(invite.secret);
            try {
              const tx = await Cipher.derive(
                secret,
                invite.room_id,
                hello,
                host,
                "client",
              );
              const rx = await Cipher.derive(
                secret,
                invite.room_id,
                hello,
                host,
                "host",
              );
              if (this.dead) {
                tx.destroy();
                rx.destroy();
                return;
              }
              this.tx = tx;
              this.rx = rx;
            } finally {
              secret.fill(0);
              this.invite = { ...invite, secret: "", client_token: "" };
            }
            this.emit([this.session.start()]);
          } else {
            this.decoder.feed(await this.rx!.open(bytes));
          }
        })
        .catch((error) => this.close(errorOf(error)))
        .finally(() => {
          this.inboundSize -= bytes.length;
          this.inboundCount--;
        });
    };
    socket.onerror = () =>
      this.close(
        new Error(
          "Relay connection failed. Check the invitation, relay origin policy, and host.",
        ),
      );
    socket.onclose = () =>
      this.close(
        new Error("Session closed. Create a fresh invitation to reconnect."),
      );
  }
  static async connect(
    invite: Invite,
    onClose: (reason: Error) => void = () => {},
    signal?: AbortSignal,
  ): Promise<Link> {
    if (signal?.aborted) throw new Error("Connection cancelled");
    const socket = new WebSocket(websocketURL(invite), [
      "lightremote.v1",
      `client.${invite.client_token}`,
    ]);
    const link = new Link(socket, invite, onClose);
    const abort = () => link.close(new Error("Connection cancelled"));
    signal?.addEventListener("abort", abort, { once: true });
    try {
      await link.ready.promise;
      return link;
    } finally {
      signal?.removeEventListener("abort", abort);
    }
  }
  get draining(): boolean {
    return this.session.draining;
  }
  get profiles(): readonly string[] {
    return this.session.negotiatedProfiles;
  }
  async open(route: string, metadata: unknown = {}): Promise<MuxStream> {
    if (this.dead) throw new Error("Session closed");
    if (this.draining)
      throw new Error("Session is draining; new streams are refused");
    const profile = `lightremote.${route.startsWith("file.") ? "file" : route}.v1`;
    const bytes = new TextEncoder().encode(JSON.stringify(metadata));
    if (bytes.length > 4096)
      throw new Error("OPEN metadata exceeds 4096 bytes");
    const { id, packet } = this.session.open({
      $typeName: "tmp.v1.Open",
      route,
      profile,
      class: route.startsWith("file.")
        ? Class.BULK
        : route === "desktop"
          ? Class.REALTIME
          : Class.INTERACTIVE,
      metadata: bytes,
    });
    const stream = new MuxStream(id, this);
    this.streams.set(id, stream);
    this.emit([packet]);
    const timeout = setTimeout(
      () => stream.reset("Stream open timed out"),
      15_000,
    );
    try {
      await stream.accepted.promise;
      return stream;
    } finally {
      clearTimeout(timeout);
    }
  }
  private receive(packet: Packet): void {
    if (this.dead) return;
    const result = this.session.receive(packet);
    // FIFO: protocol responses always precede packets caused by application events.
    this.emit(result.outbound);
    for (const event of result.events) this.event(event);
    // Includes WINDOW, which intentionally has no application event.
    this.pump();
    this.checkDrain();
  }
  private event(event: Event): void {
    if (event.type === "ready") {
      clearTimeout(this.timeout);
      this.ready.resolve();
    } else if (event.type === "open")
      this.emit(
        this.session.reset(
          event.id,
          ErrorCode.UNSUPPORTED,
          "Controller accepts no host-initiated streams",
        ),
      );
    else if (event.type === "goaway") {
      this.drainReason = event.reason || "GOAWAY";
      this.drainDeadline ??= setTimeout(
        () => this.close(new Error("Host session drain timed out")),
        15_000,
      );
    } else if (event.type === "pong") {
      if (this.drainProbe !== undefined && event.token === this.drainProbe)
        this.drainAcknowledged = true;
    } else if ("id" in event) {
      const stream = this.streams.get(event.id);
      if (!stream) return;
      if (event.type === "accepted") {
        stream.acked = true;
        stream.accepted.resolve();
      } else if (event.type === "data") stream.receive(event.bytes, event.fin);
      else if (event.type === "reset") {
        this.streams.delete(stream.id);
        stream.fail(
          new Error(event.reason || `Host reset stream (${event.code})`),
        );
      }
    }
  }
  consume(stream: MuxStream, count: number): void {
    if (this.dead || !this.streams.has(stream.id)) return;
    if (count) this.emit(this.session.consume(stream.id, count));
    this.retire(stream);
    this.pump();
  }
  retire(stream: MuxStream): void {
    if (
      stream.localFin &&
      stream.remoteFinConsumed &&
      stream.outstanding === 0
    ) {
      this.streams.delete(stream.id);
      this.checkDrain();
    }
  }
  private checkDrain(): void {
    if (this.dead || !this.draining || this.streams.size !== 0) return;
    if (this.drainProbe === undefined) {
      const random = crypto.getRandomValues(new Uint8Array(8));
      this.drainProbe = new DataView(random.buffer).getBigUint64(0);
      // FIFO places all final DATA/FIN/WINDOW packets before this end-to-end barrier.
      this.emit([this.session.ping(this.drainProbe)]);
      return;
    }
    if (!this.drainAcknowledged || this.sending || this.outboundSize !== 0)
      return;
    if (this.socket.bufferedAmount !== 0) {
      this.drainPoll ??= setTimeout(() => {
        this.drainPoll = undefined;
        this.checkDrain();
      }, 10);
      return;
    }
    this.close(
      new Error(`Host ended the session: ${this.drainReason || "GOAWAY"}`),
    );
  }
  reset(stream: MuxStream, reason: string): void {
    if (!this.streams.delete(stream.id)) return;
    // The SDK reaps a fully half-closed stream before a zero-byte FIN handler completes.
    const wireClosed =
      stream.localFin && stream.remoteFin && stream.outstanding === 0;
    if (!this.dead && !wireClosed)
      this.emit(
        this.session.reset(
          stream.id,
          ErrorCode.CANCELLED,
          reason.slice(0, 120),
        ),
      );
    stream.fail(new Error(reason));
    this.pump();
    this.checkDrain();
  }
  pump(): void {
    if (this.dead || this.pumping) return;
    this.pumping = true;
    let progressed = false,
      budget = 64;
    try {
      do {
        progressed = false;
        for (const stream of this.streams.values()) {
          if (budget <= 0 || this.outboundSize > CONNECTION_WINDOW) break;
          const write = stream.writes[0];
          if (!write || !stream.acked) continue;
          const peer = this.session.peerSettings!;
          const n = Math.min(
            write.bytes.length - write.offset,
            peer.maxData,
            peer.streamWindow,
            16384,
          );
          const fin = write.fin && write.offset + n === write.bytes.length;
          let packet: Packet;
          try {
            packet = this.session.send(
              stream.id,
              write.bytes.subarray(write.offset, write.offset + n),
              fin,
            );
          } catch (error) {
            if (error instanceof Error && error.name === "WouldBlock") continue;
            throw error;
          }
          this.emit([packet]);
          write.offset += n;
          stream.queuedSend -= n;
          progressed = true;
          budget--;
          if (write.offset === write.bytes.length) {
            stream.writes.shift();
            if (fin) stream.localFin = true;
            write.done.resolve();
            this.retire(stream);
          }
        }
      } while (
        progressed &&
        budget > 0 &&
        this.outboundSize <= CONNECTION_WINDOW
      );
    } catch (error) {
      this.close(errorOf(error));
    } finally {
      this.pumping = false;
    }
    if (progressed && budget === 0 && !this.pumpScheduled) {
      this.pumpScheduled = true;
      queueMicrotask(() => {
        this.pumpScheduled = false;
        this.pump();
      });
    }
  }
  private emit(packets: Packet[]): void {
    if (this.dead) return;
    for (const packet of packets) {
      const bytes = encode(packet);
      if (
        this.outboundSize + bytes.length > MAX_QUEUE ||
        this.outbound.length >= 4096
      ) {
        this.close(new Error("Carrier send queue exceeded"));
        return;
      }
      this.outbound.push(bytes);
      this.outboundSize += bytes.length;
    }
    void this.flush();
  }
  private async flush(): Promise<void> {
    if (this.sending || !this.tx || this.dead) return;
    this.sending = true;
    try {
      while (this.outbound.length && !this.dead) {
        const started = Date.now();
        while (this.socket.bufferedAmount > 256 * 1024 && !this.dead) {
          if (Date.now() - started > 15_000) throw new Error("Carrier stalled");
          await new Promise((resolve) => setTimeout(resolve, 10));
        }
        if (this.dead) break;
        const bytes = this.outbound.shift()!;
        const record = await this.tx.seal(bytes);
        if (this.dead) break;
        this.socket.send(record);
        this.outboundSize -= bytes.length;
        this.pump();
      }
    } catch (error) {
      this.close(errorOf(error));
    } finally {
      this.sending = false;
      this.checkDrain();
    }
  }
  close(reason = new Error("Disconnected")): void {
    if (this.dead) return;
    this.dead = true;
    clearTimeout(this.timeout);
    clearTimeout(this.drainDeadline);
    clearTimeout(this.drainPoll);
    this.session.close();
    this.tx?.destroy();
    this.rx?.destroy();
    this.tx = this.rx = undefined;
    this.invite = { ...this.invite, secret: "", client_token: "" };
    for (const stream of this.streams.values()) stream.fail(reason);
    this.streams.clear();
    this.outbound = [];
    this.outboundSize = 0;
    this.ready.reject(reason);
    this.socket.onmessage =
      this.socket.onopen =
      this.socket.onerror =
      this.socket.onclose =
        null;
    this.socket.close(1000, "Session ended");
    this.onClose(reason);
  }
}
