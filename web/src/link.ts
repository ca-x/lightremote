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
/** Local budgets in milliseconds; connectTimeoutMs covers the entire negotiation. */
export interface LinkOptions {
  connectTimeoutMs: number;
  openTimeoutMs: number;
  sendTimeoutMs: number;
  heartbeatIntervalMs: number;
  heartbeatTimeoutMs: number;
  degradedAfterMs: number;
  drainTimeoutMs: number;
}
const DEFAULT_OPTIONS: Readonly<LinkOptions> = {
  connectTimeoutMs: 20_000,
  openTimeoutMs: 15_000,
  sendTimeoutMs: 20_000,
  heartbeatIntervalMs: 10_000,
  heartbeatTimeoutMs: 30_000,
  degradedAfterMs: 3_000,
  drainTimeoutMs: 15_000,
};
export interface LinkHealth {
  readonly phase: "healthy" | "degraded" | "closed";
  readonly rtt_ms: number | null;
}
interface Outbound {
  bytes: Uint8Array;
  deadline: number;
  sent?: () => void;
}
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
  private outbound: Outbound[] = [];
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
  private heartbeatTimer?: ReturnType<typeof setTimeout>;
  private heartbeat?: { token: bigint; queuedAt: number; sentAt?: number; pongAt?: number };
  private sendDeadline?: ReturnType<typeof setTimeout>;
  private sendPoll?: ReturnType<typeof setTimeout>;
  private sendWake?: () => void;
  private abortCleanup?: () => void;
  private readonly openTimers = new Set<ReturnType<typeof setTimeout>>();
  private currentHealth: LinkHealth = { phase: "healthy", rtt_ms: null };
  /** Called on changes; read health immediately after subscribing for the current value. */
  onHealth?: (health: LinkHealth) => void;
  get health(): LinkHealth { return { ...this.currentHealth }; }
  private constructor(
    private socket: WebSocket,
    private invite: Invite,
    readonly onClose: (reason: Error) => void,
    private readonly options: Readonly<LinkOptions>,
  ) {
    this.timeout = setTimeout(
      () => this.close(new Error("Encrypted TMP handshake timed out")),
      options.connectTimeoutMs,
    );
    const hello = makeHello("client");
    socket.binaryType = "arraybuffer";
    socket.onopen = () => {
      if (socket.protocol !== "lightremote.v1") {
        this.close(new Error("Relay did not select lightremote.v1"));
        return;
      }
      try {
        socket.send(new TextEncoder().encode(JSON.stringify(hello)));
      } catch (error) {
        this.close(errorOf(error));
      }
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
    options: Partial<LinkOptions> = {},
  ): Promise<Link> {
    if (signal?.aborted) throw new Error("Connection cancelled");
    const budgets = { ...DEFAULT_OPTIONS, ...options };
    if (Object.values(budgets).some(value => !Number.isFinite(value) || value < 1 || value > 300_000)
      || budgets.degradedAfterMs > budgets.heartbeatTimeoutMs)
      throw new Error("Link budgets must be 1..300000ms, with degradation before heartbeat timeout");
    const socket = new WebSocket(websocketURL(invite), [
      "lightremote.v1",
      `client.${invite.client_token}`,
    ]);
    const link = new Link(socket, invite, onClose, budgets);
    const abort = () => link.close(new Error("Connection cancelled"));
    signal?.addEventListener("abort", abort, { once: true });
    link.abortCleanup = () => signal?.removeEventListener("abort", abort);
    if (signal?.aborted) abort();
    await link.ready.promise;
    return link;
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
      this.options.openTimeoutMs,
    );
    this.openTimers.add(timeout);
    try {
      await stream.accepted.promise;
      return stream;
    } finally {
      clearTimeout(timeout);
      this.openTimers.delete(timeout);
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
      this.armHeartbeat(this.options.heartbeatIntervalMs);
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
      clearTimeout(this.heartbeatTimer);
      this.heartbeat = undefined;
      this.drainReason = event.reason || "GOAWAY";
      this.drainDeadline ??= setTimeout(
        () => this.close(new Error("Host session drain timed out")),
        this.options.drainTimeoutMs,
      );
    } else if (event.type === "pong") {
      if (this.drainProbe !== undefined && event.token === this.drainProbe)
        this.drainAcknowledged = true;
      this.acceptHeartbeatPong(event.token, performance.now());
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
  private setHealth(health: LinkHealth): void {
    if (health.phase === this.currentHealth.phase && health.rtt_ms === this.currentHealth.rtt_ms) return;
    this.currentHealth = health;
    // A view callback must not stop carrier cleanup or turn a valid PONG into failure.
    try { this.onHealth?.(this.health); } catch { /* Observer errors do not own the link. */ }
  }
  private armHeartbeat(delay: number): void {
    clearTimeout(this.heartbeatTimer);
    if (this.dead || this.draining) return;
    this.heartbeatTimer = setTimeout(() => this.probe(), Math.max(1, delay));
  }
  private acceptHeartbeatPong(token: bigint, receivedAt: number): void {
    const pending = this.heartbeat;
    if (!pending || pending.token !== token) return;
    if (pending.sentAt === undefined) {
      // A reply can arrive before the polling task observes browser drain.
      pending.pongAt ??= receivedAt;
      return;
    }
    const rtt = Math.max(0, Math.round(receivedAt - pending.sentAt));
    this.heartbeat = undefined;
    this.setHealth({ phase: rtt >= this.options.degradedAfterMs ? "degraded" : "healthy", rtt_ms: rtt });
    this.armHeartbeat(this.options.heartbeatIntervalMs);
  }
  private probe(): void {
    if (this.dead || this.draining) return;
    const now = performance.now();
    if (!this.heartbeat) {
      const token = new DataView(crypto.getRandomValues(new Uint8Array(8)).buffer).getBigUint64(0);
      const probe = { token, queuedAt: now, sentAt: undefined as number | undefined,
        pongAt: undefined as number | undefined };
      this.heartbeat = probe;
      this.emit([this.session.ping(token)], () => {
        probe.sentAt = performance.now();
        if (this.heartbeat === probe) {
          if (probe.pongAt !== undefined) this.acceptHeartbeatPong(token, probe.pongAt);
          else this.armHeartbeat(this.options.degradedAfterMs);
        }
      });
    }
    const pending = this.heartbeat;
    if (!pending || this.dead) return;
    const elapsed = now - (pending.sentAt ?? pending.queuedAt);
    const budget = pending.sentAt === undefined ? this.options.sendTimeoutMs : this.options.heartbeatTimeoutMs;
    if (elapsed >= budget) {
      this.close(new Error(pending.sentAt === undefined ? "Heartbeat carrier queue timed out" : "Peer heartbeat timed out"));
      return;
    }
    if (elapsed >= this.options.degradedAfterMs)
      this.setHealth({ ...this.currentHealth, phase: "degraded" });
    this.armHeartbeat(Math.min(budget - elapsed,
      elapsed < this.options.degradedAfterMs ? this.options.degradedAfterMs - elapsed : budget - elapsed));
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
  private emit(packets: Packet[], sent?: () => void): void {
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
      this.outbound.push({ bytes, deadline: performance.now() + this.options.sendTimeoutMs, sent });
      this.outboundSize += bytes.length;
    }
    void this.flush();
  }
  private async waitForBrowserDrain(deadline: number, limit = 0): Promise<void> {
    let delay = 1;
    while (!this.dead && this.socket.bufferedAmount > limit) {
      const remaining = deadline - performance.now();
      if (remaining <= 0) throw new Error("Carrier send timed out");
      await new Promise<void>((resolve) => {
        this.sendWake = resolve;
        this.sendPoll = setTimeout(() => {
          this.sendPoll = undefined;
          this.sendWake = undefined;
          resolve();
        }, Math.min(delay, remaining));
      });
      delay = Math.min(delay * 2, 10);
    }
  }
  private async flush(): Promise<void> {
    if (this.sending || !this.tx || this.dead) return;
    this.sending = true;
    try {
      while (this.outbound.length && !this.dead) {
        const item = this.outbound[0];
        const remaining = item.deadline - performance.now();
        if (remaining <= 0) throw new Error("Carrier send queue timed out");
        this.sendDeadline = setTimeout(() => this.close(new Error("Carrier send timed out")), remaining);
        await this.waitForBrowserDrain(item.deadline, 256 * 1024);
        if (this.dead) break;
        const record = await this.tx.seal(item.bytes);
        if (this.dead) break;
        if (performance.now() >= item.deadline) throw new Error("Carrier send queue timed out");
        this.socket.send(record);
        // send() only enqueues bytes in the browser; retain this record's deadline
        // and FIFO slot until those bytes have left the browser's outgoing queue.
        await this.waitForBrowserDrain(item.deadline);
        if (this.dead) break;
        if (performance.now() >= item.deadline) throw new Error("Carrier send timed out");
        clearTimeout(this.sendDeadline);
        this.sendDeadline = undefined;
        this.outbound.shift();
        this.outboundSize -= item.bytes.length;
        item.sent?.();
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
    clearTimeout(this.heartbeatTimer);
    clearTimeout(this.sendDeadline);
    clearTimeout(this.sendPoll);
    this.sendWake?.();
    this.sendWake = undefined;
    this.heartbeat = undefined;
    for (const timer of this.openTimers) clearTimeout(timer);
    this.openTimers.clear();
    this.abortCleanup?.();
    this.abortCleanup = undefined;
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
    try { this.socket.close(1000, "Session ended"); } catch { /* Already closed. */ }
    this.setHealth({ ...this.currentHealth, phase: "closed" });
    this.onClose(reason);
  }
}
