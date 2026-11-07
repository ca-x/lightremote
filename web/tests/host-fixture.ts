/** Test-only encrypted TMP host. Fixtures verify browser wiring, never physical capture or a PTY. */
import { WebSocketServer, type WebSocket } from "ws";
import { createHash } from "node:crypto";
import {
  Session,
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
  base64url,
  type Invite,
} from "../src/secure";
import { Records, jsonFrame, parseJSON } from "../src/records";
const profiles = ["control", "desktop", "input", "shell", "file"].map(
  (n) => `lightremote.${n}.v1`,
);
import {
  scopes,
  type Scope,
  type PermissionSnapshot,
} from "../src/permissions";
const enc = new TextEncoder();
interface State {
  route: string;
  records: Records;
  bytes: Uint8Array[];
  path: string;
  size: number;
}
export class HostFixture {
  readonly server = new WebSocketServer({
    host: "127.0.0.1",
    port: 0,
    handleProtocols: (protocols) =>
      protocols.has("lightremote.v1") && "lightremote.v1",
  });
  readonly key = new Uint8Array(32).fill(37);
  readonly files = new Map<string, Uint8Array>([
    ["hello.txt", enc.encode("hello from fixture\n")],
    ["large.bin", new Uint8Array(700000).map((_, i) => i % 251)],
  ]);
  readonly inputs: unknown[] = [];
  readonly desktopRequests: {
    display: number;
    fps: number;
    quality: number;
    single: boolean;
  }[] = [];
  readonly calls: { method: string; params: Record<string, unknown> }[] = [];
  readonly errors: Error[] = [];
  permissions: PermissionSnapshot = {
    available: [...scopes],
    granted: [...scopes],
    pending: null,
    last_decision: null,
    revision: 0,
    mode: "unattended",
  };
  private nextRequest = 1;
  private permissionSenders = new Set<() => void>();
  setAttended() {
    this.permissions = {
      ...this.permissions,
      granted: [],
      mode: "attended",
      revision: this.permissions.revision + 1,
    };
  }
  approve() {
    this.setPermissions(
      [
        ...new Set([
          ...this.permissions.granted,
          ...(this.permissions.pending?.scopes ?? []),
        ]),
      ],
      "approved",
    );
  }
  deny() {
    this.setPermissions(this.permissions.granted, "denied");
  }
  revoke(scopes: Scope[]) {
    this.setPermissions(
      this.permissions.granted.filter((scope) => !scopes.includes(scope)),
    );
  }
  private setPermissions(granted: Scope[], outcome?: "approved" | "denied") {
    const decision =
      outcome && this.permissions.pending
        ? { id: this.permissions.pending.id, outcome }
        : this.permissions.last_decision;
    this.permissions = {
      ...this.permissions,
      granted,
      last_decision: decision,
      pending: null,
      revision: this.permissions.revision + 1,
    };
    for (const send of this.permissionSenders) send();
  }
  withholdCredit = false;
  holdPongs = false;
  pings = 0;
  connectionCreditReturned = 0;
  private goawaySenders = new Set<() => void>();
  private controlEnders = new Set<() => void>();
  endControl() {
    for (const end of this.controlEnders) end();
  }
  private heldPongs: (() => void)[] = [];
  goaway() {
    for (const send of this.goawaySenders) send();
  }
  releasePongs() {
    for (const send of this.heldPongs.splice(0)) send();
  }
  shellInput = "";
  windows = 0;
  resets = 0;
  remoteFins = 0;
  clipboard = "fixture clipboard";
  desktopJPEG?: Uint8Array;
  private sockets = new Set<WebSocket>();
  private constructor() {
    this.server.on("connection", (socket, request) => {
      if (
        !request.headers["sec-websocket-protocol"]?.includes(
          "client.fixture_ticket",
        ) ||
        request.url !== "/v1/rooms/fixture_room/ws"
      ) {
        socket.close(1008);
        return;
      }
      this.sockets.add(socket);
      socket.on("close", () => this.sockets.delete(socket));
      this.serve(socket);
    });
  }
  static async start() {
    const fixture = new HostFixture();
    await new Promise<void>((resolve) =>
      fixture.server.on("listening", resolve),
    );
    return fixture;
  }
  invite(): Invite {
    const address = this.server.address();
    if (!address || typeof address === "string")
      throw new Error("Fixture unavailable");
    return {
      relay: `http://127.0.0.1:${address.port}`,
      room_id: "fixture_room",
      client_token: "fixture_ticket",
      secret: base64url(this.key),
    };
  }
  fragment() {
    return base64url(enc.encode(JSON.stringify(this.invite())));
  }
  async close() {
    for (const socket of this.sockets) socket.terminate();
    await new Promise<void>((resolve) => this.server.close(() => resolve()));
  }
  private serve(socket: WebSocket) {
    const session = new Session(
      "responder",
      {
        maxData: 4096,
        streamWindow: 8192,
        connectionWindow: 65536,
        maxStreams: 16,
      },
      profiles,
    );
    const host = makeHello("host");
    let tx: Cipher, rx: Cipher;
    let incoming = Promise.resolve(),
      sending = Promise.resolve();
    const states = new Map<number, State>();
    const pending = new Map<
      number,
      { bytes: Uint8Array; offset: number; fin: boolean }[]
    >();
    const send = (packets: Packet[]) => {
      if (!packets.length) return;
      // Coalesce multiple packets, then split TMP framing across encrypted records.
      const frames = packets.map(encode);
      const data = Buffer.concat(frames);
      sending = sending
        .then(async () => {
          let offset = 0;
          for (const n of [1, 2, 5, data.length]) {
            if (offset >= data.length) break;
            const end = Math.min(data.length, offset + n);
            socket.send(await tx.seal(data.subarray(offset, end)));
            offset = end;
          }
        })
        .catch((error) => {
          if (socket.readyState === socket.OPEN) {
            this.errors.push(error);
            socket.close(1011);
          }
        });
    };
    const queue = (id: number, bytes: Uint8Array, fin = false) => {
      const list = pending.get(id) || [];
      list.push({ bytes, offset: 0, fin });
      pending.set(id, list);
    };
    const pump = () => {
      for (const [id, list] of pending) {
        while (list.length) {
          const item = list[0],
            n = Math.min(4096, item.bytes.length - item.offset);
          try {
            send([
              session.send(
                id,
                item.bytes.subarray(item.offset, item.offset + n),
                item.fin && item.offset + n === item.bytes.length,
              ),
            ]);
          } catch (error) {
            if (error instanceof Error && error.name === "WouldBlock") break;
            throw error;
          }
          item.offset += n;
          if (item.offset === item.bytes.length) list.shift();
        }
        if (!list.length) pending.delete(id);
      }
    };
    const goaway = () => {
      send(session.goaway(ErrorCode.CANCELLED, "Fixture drain"));
      pump();
    };
    this.goawaySenders.add(goaway);
    const endControl = () => {
      for (const [id, state] of states)
        if (state.route === "control") queue(id, new Uint8Array(), true);
      pump();
    };
    this.controlEnders.add(endControl);
    const reply = (id: number, value: unknown) => queue(id, jsonFrame(value));
    const permissionsChanged = () => {
      for (const [id, state] of states)
        if (state.route === "control")
          reply(id, { event: "permissions.changed", data: this.permissions });
      pump();
    };
    this.permissionSenders.add(permissionsChanged);
    const event = async (event: Event) => {
      if (event.type === "open") {
        const metadata = JSON.parse(
          new TextDecoder().decode(event.open.metadata),
        );
        if (event.open.route === "desktop") this.desktopRequests.push(metadata);
        if (event.open.route === "desktop" && !this.desktopJPEG) {
          send(
            session.reset(
              event.id,
              ErrorCode.UNSUPPORTED,
              "Desktop capture unavailable on headless host",
            ),
          );
          return;
        }
        states.set(event.id, {
          route: event.open.route,
          records: new Records(1024 * 1024),
          bytes: [],
          path: metadata.path,
          size: metadata.size,
        });
        send([session.accept(event.id)]);
        if (event.open.route === "shell")
          queue(event.id, enc.encode("fixture-shell> "));
        if (event.open.route === "desktop")
          queue(
            event.id,
            Buffer.concat([jsonLength(this.desktopJPEG!), this.desktopJPEG!]),
            !!metadata.single,
          );
      } else if (event.type === "data") {
        const state = states.get(event.id);
        if (!state) return;
        if (state.route === "control")
          await state.records.feed(event.bytes, (body) => {
            const message = parseJSON(body) as {
              id: number;
              method: string;
              params: Record<string, unknown>;
            };
            this.calls.push(message);
            let result: unknown = {};
            const granted = (scope: Scope) =>
              this.permissions.granted.includes(scope);
            if (message.method === "capabilities")
              result = {
                ...Object.fromEntries(
                  scopes.map((scope) => [scope, granted(scope)]),
                ),
                files: granted("file_read"),
                clipboard:
                  granted("clipboard_read") || granted("clipboard_write"),
                permissions: this.permissions,
                routes: [
                  "control",
                  ...(granted("desktop") ? ["desktop"] : []),
                  ...(granted("input") ? ["input"] : []),
                  ...(granted("shell") ? ["shell"] : []),
                  ...(granted("file_read") ? ["file.get"] : []),
                  ...(granted("file_write") ? ["file.put"] : []),
                ],
              };
            else if (message.method === "permissions.get")
              result = this.permissions;
            else if (message.method === "permissions.request") {
              const requested = message.params.scopes as Scope[];
              if (
                this.permissions.pending ||
                !Array.isArray(requested) ||
                !requested.length ||
                requested.some(
                  (scope) => !this.permissions.available.includes(scope),
                )
              ) {
                reply(event.id, {
                  id: message.id,
                  error: {
                    code: "invalid",
                    message: "Invalid permission request",
                  },
                });
                return;
              }
              this.permissions = {
                ...this.permissions,
                pending: { id: this.nextRequest++, scopes: requested },
                last_decision: null,
                revision: this.permissions.revision + 1,
              };
              result = this.permissions;
            } else if (message.method === "permissions.cancel") {
              if (
                this.permissions.pending &&
                this.permissions.pending.id === message.params.id
              )
                this.permissions = {
                  ...this.permissions,
                  last_decision: {
                    id: this.permissions.pending.id,
                    outcome: "cancelled",
                  },
                  pending: null,
                  revision: this.permissions.revision + 1,
                };
              result = this.permissions;
            } else if (message.method === "fs.list")
              result = {
                entries: [...this.files].map(([name, bytes]) => ({
                  name,
                  type: "file",
                  size: bytes.length,
                })),
              };
            else if (message.method === "desktop.displays")
              result = {
                displays: [
                  { id: 0, name: "Fixture display", width: 16, height: 12 },
                ],
              };
            else if (message.method === "clipboard.get")
              result = { text: this.clipboard };
            else if (message.method === "clipboard.set")
              this.clipboard = String(message.params.text);
            reply(event.id, { id: message.id, result });
          });
        else if (state.route === "shell") {
          this.shellInput += new TextDecoder().decode(event.bytes);
          queue(event.id, event.bytes);
        } else if (state.route === "input")
          await state.records.feed(event.bytes, (body) => {
            this.inputs.push(parseJSON(body));
          });
        else if (state.route === "file.put")
          state.bytes.push(Uint8Array.from(event.bytes));
        if (event.bytes.length && !this.withholdCredit)
          send(session.consume(event.id, event.bytes.length));
        if (event.fin) {
          this.remoteFins++;
          if (state.route === "file.get") {
            const bytes = this.files.get(state.path);
            if (!bytes) {
              send(
                session.reset(event.id, ErrorCode.REFUSED, "File not found"),
              );
              states.delete(event.id);
            } else queue(event.id, bytes, true);
          } else if (state.route === "file.put") {
            const bytes = Buffer.concat(state.bytes);
            if (bytes.length !== state.size || this.files.has(state.path)) {
              send(
                session.reset(event.id, ErrorCode.REFUSED, "Upload rejected"),
              );
            } else {
              this.files.set(state.path, bytes);
              queue(
                event.id,
                jsonFrame({
                  size: bytes.length,
                  sha256: createHash("sha256").update(bytes).digest("hex"),
                }),
                true,
              );
            }
          }
        }
      } else if (event.type === "reset") {
        this.resets++;
        states.delete(event.id);
        pending.delete(event.id);
      }
    };
    const packets: Packet[] = [];
    const decoder = new IncrementalDecoder((packet) => packets.push(packet));
    socket.on("message", (raw, binary) => {
      incoming = incoming
        .then(async () => {
          if (!binary) throw new Error("Nonbinary carrier");
          const bytes = Buffer.from(raw as Buffer);
          if (!rx) {
            const client = parseHello(bytes, "client");
            tx = await Cipher.derive(
              this.key,
              "fixture_room",
              client,
              host,
              "host",
            );
            rx = await Cipher.derive(
              this.key,
              "fixture_room",
              client,
              host,
              "client",
            );
            socket.send(enc.encode(JSON.stringify(host)));
            return;
          }
          decoder.feed(await rx.open(bytes));
          for (const packet of packets.splice(0)) {
            if (packet.body.case === "window") {
              this.windows++;
              if (packet.streamId === 0)
                this.connectionCreditReturned += packet.body.value.credit;
            }
            if (packet.body.case === "ping") this.pings++;
            const result = session.receive(packet);
            for (const response of result.outbound) {
              if (response.body.case === "pong" && this.holdPongs)
                this.heldPongs.push(() => send([response]));
              else send([response]);
            }
            for (const item of result.events) await event(item);
            pump();
          }
        })
        .catch((error) => {
          this.errors.push(error);
          socket.close(1011);
        });
    });
    socket.on("close", () => {
      this.goawaySenders.delete(goaway);
      this.controlEnders.delete(endControl);
      this.permissionSenders.delete(permissionsChanged);
      session.close();
      tx?.destroy();
      rx?.destroy();
    });
  }
}
function jsonLength(bytes: Uint8Array) {
  const b = Buffer.alloc(4);
  b.writeUInt32BE(bytes.length);
  return b;
}
