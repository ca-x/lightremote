// No credential, URL, frame or payload logging. The fixed singleton DO is also the
// bounded room registry: arbitrary room IDs cannot allocate additional objects.
export interface Env {
  RELAY: DurableObjectNamespace;
  LIGHTREMOTE_RELAY_TOKEN: string;
  LIGHTREMOTE_MAX_ROOMS?: string;
  LIGHTREMOTE_ROOM_TTL_SECONDS?: string;
  LIGHTREMOTE_WAIT_SECONDS?: string;
  LIGHTREMOTE_SESSION_BYTES?: string;
  LIGHTREMOTE_ALLOWED_ORIGINS?: string;
  LIGHTREMOTE_PUBLIC_ORIGIN?: string;
}
const MAX_RECORD = 150_000;
const QUEUE_RECORDS = 8;
const CHUNK_SIZE = 60_000;
const MAX_SESSION_RECORDS = 262_144;
interface Limits { rooms: number; ttl: number; wait: number; bytes: number }
interface Attachment { room: string; role: 0 | 1; version: 1 }
interface Room {
  id: string;
  hashes: [string, string];
  joined: [boolean, boolean];
  expires: number;
  waitingUntil?: number;
  pending: [number[], number[]]; // record lengths, each record stored in <=60KB chunks
  sequence: [number, number];
  forwarded: number;
  records: number;
  busy: boolean;
}
function limits(env: Env): Limits {
  const number = (v: string | undefined, fallback: number, max: number): number => {
    const n = v === undefined ? fallback : Number(v);
    if (!Number.isSafeInteger(n) || n < 1 || n > max) throw new Error("Invalid relay limits");
    return n;
  };
  return {
    rooms: number(env.LIGHTREMOTE_MAX_ROOMS, 128, 4096),
    ttl: number(env.LIGHTREMOTE_ROOM_TTL_SECONDS, 3600, 86400) * 1000,
    wait: number(env.LIGHTREMOTE_WAIT_SECONDS, 30, 86400) * 1000,
    bytes: number(env.LIGHTREMOTE_SESSION_BYTES, 64 * 1024 * 1024, 256 * 1024 * 1024),
  };
}
function originValue(value: string): string | undefined {
  try {
    const u = new URL(value);
    if (!['http:', 'https:'].includes(u.protocol) || u.username || u.password || u.pathname !== '/' || u.search || u.hash) return;
    return u.origin;
  } catch { return; }
}
function originAllowed(request: Request, env: Env): boolean {
  const raw = request.headers.get('Origin');
  if (raw === null) return true; // Native clients have no Origin; tickets remain mandatory.
  const origin = originValue(raw);
  if (!origin) return false;
  const allow = (env.LIGHTREMOTE_ALLOWED_ORIGINS ?? '').split(',').map(s => s.trim()).filter(Boolean);
  if (allow.length) return allow.some(o => originValue(o) === origin);
  const host = new URL(origin).hostname;
  return ['localhost', '127.0.0.1', '[::1]'].includes(host) || origin === (env.LIGHTREMOTE_PUBLIC_ORIGIN ? originValue(env.LIGHTREMOTE_PUBLIC_ORIGIN) : new URL(request.url).origin);
}
function response(status: number): Response { return new Response(null, { status, headers: { 'Cache-Control': 'no-store' } }); }
async function digest(value: string): Promise<string> {
  return Array.from(new Uint8Array(await crypto.subtle.digest('SHA-256', new TextEncoder().encode(value))), b => b.toString(16).padStart(2, '0')).join('');
}
function equal(a: string, b: string): boolean {
  if (a.length !== b.length) return false;
  let diff = 0; for (let i = 0; i < a.length; i++) diff |= a.charCodeAt(i) ^ b.charCodeAt(i);
  return diff === 0;
}
function token(): string {
  return btoa(String.fromCharCode(...crypto.getRandomValues(new Uint8Array(32)))).replaceAll('+', '-').replaceAll('/', '_').replaceAll('=', '');
}
function credentials(request: Request): { role: 0 | 1; ticket: string } | undefined {
  const parts = (request.headers.get('Sec-WebSocket-Protocol') ?? '').split(',').map(s => s.trim());
  if (parts.length !== 2 || !parts.includes('lightremote.v1')) return;
  const ticket = parts.find(p => p !== 'lightremote.v1');
  const match = /^(host|client)\.([A-Za-z0-9_-]{43})$/.exec(ticket ?? '');
  if (match) return { role: match[1] === 'host' ? 0 : 1, ticket: match[2] };
}
function attachment(socket: WebSocket): Attachment | undefined {
  try {
    const a: unknown = socket.deserializeAttachment();
    if (typeof a !== 'object' || !a) return;
    const v = a as Partial<Attachment>;
    if (v.version !== 1 || typeof v.room !== 'string' || !/^[A-Za-z0-9_-]{43}$/.test(v.room) || (v.role !== 0 && v.role !== 1)) return;
    return v as Attachment;
  } catch { return; }
}
function close(socket: WebSocket, code: number, reason: string): void {
  try { socket.close(code, reason); } catch { /* A failed socket cannot be reused. */ }
}

function corsResponse(request: Request, original: Response): Response {
  let result = original;
  // Upgrade responses preserve their websocket; do not reconstruct them as HTTP.
  if (result.status !== 101) {
    result = new Response(result.body, result);
    result.headers.set('Cache-Control', 'no-store');
    const origin = request.headers.get('Origin');
    if (origin) {
      result.headers.set('Access-Control-Allow-Origin', origin);
      result.headers.set('Vary', 'Origin');
      result.headers.set('Access-Control-Allow-Methods', 'GET, POST, OPTIONS');
      result.headers.set('Access-Control-Allow-Headers', 'Authorization, Content-Type');
    }
  }
  return result;
}

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    // Fail closed even on health if the mandatory operator secret/config is absent.
    if (!env.LIGHTREMOTE_RELAY_TOKEN?.trim()) return response(503);
    try { limits(env); } catch { return response(503); }
    const configuredOrigins = [...(env.LIGHTREMOTE_ALLOWED_ORIGINS ?? '').split(',').map(s => s.trim()).filter(Boolean), ...(env.LIGHTREMOTE_PUBLIC_ORIGIN ? [env.LIGHTREMOTE_PUBLIC_ORIGIN] : [])];
    if (configuredOrigins.some(o => !originValue(o))) return response(503);
    if (!originAllowed(request, env)) return response(403);
    const url = new URL(request.url);
    let result: Response;
    if (request.method === 'OPTIONS') result = response(204);
    else if (url.pathname === '/healthz' && request.method === 'GET') result = Response.json({ ok: true, protocol: 'lightremote-relay.v1' });
    else if (url.pathname === '/v1/rooms' && request.method === 'POST') {
      const authorization = request.headers.get('Authorization');
      const bearer = authorization?.startsWith('Bearer ') ? authorization.slice(7) : undefined;
      if (!bearer || !equal(await digest(bearer), await digest(env.LIGHTREMOTE_RELAY_TOKEN))) return corsResponse(request, response(401));
      // Body read by DO under the 1024-byte cap. Auth precedes any allocation.
      result = await env.RELAY.get(env.RELAY.idFromName('lightremote-relay.v1')).fetch(request);
    } else if (/^\/v1\/rooms\/[A-Za-z0-9_-]{43}\/ws$/.test(url.pathname) && request.method === 'GET') {
      if (url.search) return corsResponse(request, response(400));
      if (!credentials(request)) return corsResponse(request, response(401));
      result = await env.RELAY.get(env.RELAY.idFromName('lightremote-relay.v1')).fetch(request);
    } else result = response(404);

    return corsResponse(request, result);
  },
} satisfies ExportedHandler<Env>;

export class RelayRooms {
  private rooms = new Map<string, Room>();
  private readonly config: Limits;
  constructor(private readonly ctx: DurableObjectState, private readonly env: Env) {
    this.config = limits(env);
    ctx.blockConcurrencyWhile(async () => {
      this.rooms = new Map(Array.from(await ctx.storage.list<Room>({ prefix: 'room:' }), ([, r]) => [r.id, r]));
      const sockets = ctx.getWebSockets();
      for (const socket of sockets) {
        const a = attachment(socket);
        if (!a || !this.rooms.has(a.room)) close(socket, 1008, 'invalid restored session');
      }
      // Restore only when every claimed role has exactly one matching attachment.
      // An interrupted join, lost socket or inconsistent crash state consumes the room.
      for (const room of this.rooms.values()) {
        const counts = [0, 0];
        for (const socket of sockets) { const a = attachment(socket); if (a?.room === room.id) counts[a.role]++; }
        const consistent = room.joined.every((joined, role) => counts[role] === (joined ? 1 : 0));
        if (room.busy || !consistent || room.expires <= Date.now() || (room.waitingUntil !== undefined && room.waitingUntil <= Date.now())) await this.consume(room.id, 1008, 'session unavailable');
      }
      await this.schedule();
    });
  }
  async fetch(request: Request): Promise<Response> {
    return this.ctx.blockConcurrencyWhile(async () => {
      if (!this.env.LIGHTREMOTE_RELAY_TOKEN?.trim() || !originAllowed(request, this.env)) return response(403);
      const url = new URL(request.url);
      await this.expire();
      if (url.pathname === '/v1/rooms' && request.method === 'POST') {
        const bearer = request.headers.get('Authorization');
        if (!bearer || !equal(await digest(bearer), await digest(`Bearer ${this.env.LIGHTREMOTE_RELAY_TOKEN}`))) return response(401);
        const contentType = request.headers.get('Content-Type')?.split(';')[0].trim().toLowerCase() ?? '';
        if (contentType !== 'application/json' && !/^application\/[^;]+\+json$/.test(contentType)) return response(415);
        // Bound the streaming body before assembling/decoding it.
        const reader = request.body?.getReader(); const chunks: Uint8Array[] = []; let length = 0;
        if (reader) {
          for (;;) { const part = await reader.read(); if (part.done) break; length += part.value.length; if (length > 1024) { await reader.cancel(); return response(413); } chunks.push(part.value); }
        }
        const all = new Uint8Array(length); let offset = 0; for (const c of chunks) { all.set(c, offset); offset += c.length; }
        try {
          const value: unknown = JSON.parse(new TextDecoder().decode(all));
          if (!value || typeof value !== 'object' || Array.isArray(value) || Object.keys(value).length !== 0) return response(400);
        } catch { return response(400); }
        if (this.rooms.size >= this.config.rooms) return response(429);
        const id = token(); const host = token(); const client = token();
        const expires = Math.floor((Date.now() + this.config.ttl) / 1000) * 1000;
        const room: Room = { id, hashes: [await digest(host), await digest(client)], joined: [false, false], expires, pending: [[], []], sequence: [0, 0], forwarded: 0, records: 0, busy: false };
        await this.save(room); this.rooms.set(id, room); await this.schedule();
        return Response.json({ room_id: id, host_token: host, client_token: client, expires_at: expires / 1000 });
      }
      const match = /^\/v1\/rooms\/([A-Za-z0-9_-]{43})\/ws$/.exec(url.pathname);
      if (!match || request.method !== 'GET') return response(404);
      if (url.search) return response(400);
      if (request.headers.get('Upgrade')?.toLowerCase() !== 'websocket') return response(400);
      const auth = credentials(request); if (!auth) return response(401);
      const room = this.rooms.get(match[1]); if (!room) return response(404);
      if (!equal(await digest(auth.ticket), room.hashes[auth.role])) return response(401);
      if (room.joined[auth.role]) return response(409);
      const pair = new WebSocketPair(); const client = pair[0]; const server = pair[1];
      room.busy = true;
      room.joined[auth.role] = true;
      if (!room.waitingUntil) room.waitingUntil = Date.now() + this.config.wait;
      if (room.joined.every(Boolean)) delete room.waitingUntil;
      // Durable reservation precedes acceptance. Crash in this gap fails closed on restore.
      await this.save(room);
      this.ctx.acceptWebSocket(server, [room.id]);
      server.serializeAttachment({ room: room.id, role: auth.role, version: 1 } satisfies Attachment);
      try { await this.flush(room, auth.role, server); room.busy = false; await this.save(room); } catch { await this.consume(room.id, 1011, 'session unavailable'); }
      await this.schedule();
      return new Response(null, { status: 101, webSocket: client, headers: { 'Sec-WebSocket-Protocol': 'lightremote.v1' } });
    });
  }
  async webSocketMessage(socket: WebSocket, message: string | ArrayBuffer): Promise<void> {
    await this.ctx.blockConcurrencyWhile(async () => {
      const a = attachment(socket); const room = a && this.rooms.get(a.room);
      if (!a || !room) { close(socket, 1008, 'session unavailable'); return; }
      if (room.expires <= Date.now() || (room.waitingUntil !== undefined && room.waitingUntil <= Date.now())) { await this.consume(room.id, 1008, 'session expired'); return; }
      if (typeof message === 'string') { await this.consume(room.id, 1003, 'binary records required'); return; }
      if (message.byteLength > MAX_RECORD) { await this.consume(room.id, 1009, 'record too large'); return; }
      // send() has no drain signal. Count all accepted bytes, including pre-peer
      // records, persist before send, and never replenish this lifetime budget.
      if (room.forwarded + message.byteLength > this.config.bytes) { await this.consume(room.id, 1008, 'session byte quota exhausted'); return; }
      if (room.records >= MAX_SESSION_RECORDS) { await this.consume(room.id, 1008, 'session record quota exhausted'); return; }
      room.busy = true; room.records++; room.forwarded += message.byteLength;
      await this.save(room);
      const target = (1 - a.role) as 0 | 1;
      const peer = this.ctx.getWebSockets(room.id).find(s => attachment(s)?.role === target);
      if (peer) {
        await this.save(room);
        try { peer.send(message); } catch { await this.consume(room.id, 1011, 'peer unavailable'); }
      } else {
        if (room.joined[target] || room.pending[target].length >= QUEUE_RECORDS) { await this.consume(room.id, 1008, 'queue full'); return; }
        const index = room.sequence[target] + room.pending[target].length;
        for (let offset = 0; offset < message.byteLength; offset += CHUNK_SIZE) await this.ctx.storage.put(this.chunkKey(room.id, target, index, offset), message.slice(offset, offset + CHUNK_SIZE));
        room.pending[target].push(message.byteLength); await this.save(room);
      }
      if (this.rooms.has(room.id)) { room.busy = false; await this.save(room); }
    });
  }
  async webSocketClose(socket: WebSocket): Promise<void> { await this.closed(socket); }
  async webSocketError(socket: WebSocket): Promise<void> { await this.closed(socket); }
  private async closed(socket: WebSocket): Promise<void> {
    await this.ctx.blockConcurrencyWhile(async () => { const a = attachment(socket); if (a) await this.consume(a.room, 1000, 'peer disconnected'); else close(socket, 1008, 'session unavailable'); await this.schedule(); });
  }
  async alarm(): Promise<void> { await this.ctx.blockConcurrencyWhile(async () => { await this.expire(); await this.schedule(); }); }
  private save(room: Room): Promise<void> { return this.ctx.storage.put(`room:${room.id}`, room); }
  private chunkKey(id: string, role: number, index: number, offset: number): string { return `queue:${id}:${role}:${index}:${offset}`; }
  private async flush(room: Room, role: 0 | 1, socket: WebSocket): Promise<void> {
    while (room.pending[role].length) {
      const length = room.pending[role][0]; const index = room.sequence[role]; const record = new Uint8Array(length);
      const keys: string[] = [];
      for (let offset = 0; offset < length; offset += CHUNK_SIZE) {
        const key = this.chunkKey(room.id, role, index, offset); keys.push(key);
        const chunk = await this.ctx.storage.get<ArrayBuffer>(key);
        if (!chunk || chunk.byteLength !== Math.min(CHUNK_SIZE, length - offset)) throw new Error('Incomplete durable queue');
        record.set(new Uint8Array(chunk), offset);
      }
      // Advance durable state before send: interrupted flush consumes on restore if
      // acceptance was incomplete; no ciphertext is silently replayed.
      room.pending[role].shift(); room.sequence[role]++;
      await this.save(room); if (keys.length) await this.ctx.storage.delete(keys);
      socket.send(record.buffer);
    }
  }
  private async consume(id: string, code: number, reason: string): Promise<void> {
    // Persist consumption before socket close. A consumed ID is never recreated.
    await this.ctx.storage.transaction(async tx => {
      const chunks = await tx.list({ prefix: `queue:${id}:` });
      await tx.delete([`room:${id}`, ...chunks.keys()]);
    });
    this.rooms.delete(id);
    for (const socket of this.ctx.getWebSockets(id)) close(socket, code, reason);
  }
  private async expire(): Promise<void> {
    for (const room of this.rooms.values()) if (room.expires <= Date.now() || (room.waitingUntil !== undefined && room.waitingUntil <= Date.now())) await this.consume(room.id, 1008, 'session expired');
  }
  private async schedule(): Promise<void> {
    let deadline = Infinity;
    for (const room of this.rooms.values()) deadline = Math.min(deadline, room.expires, room.waitingUntil ?? Infinity);
    if (Number.isFinite(deadline)) await this.ctx.storage.setAlarm(deadline); else await this.ctx.storage.deleteAlarm();
  }
}
