import { env, SELF, runInDurableObject, runDurableObjectAlarm } from 'cloudflare:test';
import { beforeEach, describe, expect, it } from 'vitest';
import worker, { Env, RelayRooms } from '../src/index';

declare module 'cloudflare:test' { interface ProvidedEnv extends Env {} }
interface Room { room_id: string; host_token: string; client_token: string; expires_at: number }
const base = 'https://relay.example';
async function create(): Promise<Room> {
  const response = await SELF.fetch(`${base}/v1/rooms`, { method: 'POST', headers: { Authorization: 'Bearer test-operator-secret', 'Content-Type': 'application/json' }, body: '{}' });
  expect(response.status).toBe(200); return response.json<Room>();
}
async function upgrade(room: Room, role: 'host' | 'client', token = room[`${role}_token`]): Promise<Response> {
  return SELF.fetch(`${base}/v1/rooms/${room.room_id}/ws`, { headers: { Upgrade: 'websocket', 'Sec-WebSocket-Protocol': `lightremote.v1, ${role}.${token}` } });
}
async function socket(room: Room, role: 'host' | 'client'): Promise<WebSocket> {
  const response = await upgrade(room, role);
  expect(response.status).toBe(101); expect(response.headers.get('Sec-WebSocket-Protocol')).toBe('lightremote.v1');
  const ws = response.webSocket!; ws.accept(); return ws;
}
function message(ws: WebSocket): Promise<ArrayBuffer> {
  return new Promise((resolve, reject) => { ws.addEventListener('message', e => resolve(e.data as ArrayBuffer), { once: true }); ws.addEventListener('error', reject, { once: true }); });
}
function closed(ws: WebSocket): Promise<CloseEvent> {
  return new Promise(resolve => ws.addEventListener('close', resolve, { once: true }));
}
function stub(): DurableObjectStub { return env.RELAY.get(env.RELAY.idFromName('lightremote-relay.v1')); }


// The current plugin shares a worker's durable storage within a test file.
// Reset both persisted storage and the live actor registry between scenarios.
beforeEach(async () => {
  await runInDurableObject(stub(), async (instance: RelayRooms, state) => {
    for (const socket of state.getWebSockets()) socket.close(1000, 'test reset');
    await state.storage.deleteAll();
    await state.storage.deleteAlarm();
    (instance as unknown as { rooms: Map<string, unknown> }).rooms.clear();
  });
});

describe('relay conformance against Workers runtime', () => {
  it('fails closed without operator secret or with limits over hard ceilings', async () => {
    expect((await worker.fetch(new Request(`${base}/healthz`), { ...env, LIGHTREMOTE_RELAY_TOKEN: '' })).status).toBe(503);
    expect((await worker.fetch(new Request(`${base}/healthz`), { ...env, LIGHTREMOTE_SESSION_BYTES: '268435457' })).status).toBe(503);
    expect((await worker.fetch(new Request(`${base}/healthz`), { ...env, LIGHTREMOTE_ROOM_TTL_SECONDS: '86401' })).status).toBe(503);
  });
  it('bounds room allocations including unused rooms', async () => {
    for (let i = 0; i < 8; i++) await create();
    expect((await SELF.fetch(`${base}/v1/rooms`, { method: 'POST', headers: { Authorization: 'Bearer test-operator-secret', 'Content-Type': 'application/json' }, body: '{}' })).status).toBe(429);
  });

  it('requires operator authentication before allocating and enforces origin', async () => {
    const unauthenticated = await SELF.fetch(`${base}/v1/rooms`, { method: 'POST', headers: { Origin: base }, body: '{}' });
    expect(unauthenticated.status).toBe(401);
    expect(unauthenticated.headers.get('Access-Control-Allow-Origin')).toBe(base);
    expect((await SELF.fetch(`${base}/v1/rooms`, { method: 'POST', headers: { Authorization: 'Bearer wrong' }, body: '{}' })).status).toBe(401);
    await runInDurableObject(stub(), async (_instance, state) => { expect((await state.storage.list({ prefix: 'room:' })).size).toBe(0); });
    const health = await SELF.fetch(`${base}/healthz`); expect(await health.json()).toEqual({ ok: true, protocol: 'lightremote-relay.v1' });
    expect((await SELF.fetch(`${base}/healthz`, { headers: { Origin: 'https://evil.example' } })).status).toBe(403);
    expect((await SELF.fetch(`${base}/healthz`, { headers: { Origin: 'http://127.0.0.1:5173' } })).status).toBe(200);
    const r = await create(); expect(r.host_token).toHaveLength(43); expect(r.host_token).not.toBe(r.client_token); expect(Number.isInteger(r.expires_at)).toBe(true);
  });
  it('rejects wrong ticket, duplicate role, and query tickets; forwards binary both ways', async () => {
    const r = await create();
    expect((await upgrade(r, 'host', r.client_token)).status).toBe(401);
    expect((await SELF.fetch(`${base}/v1/rooms/${r.room_id}/ws?token=${r.host_token}`, { headers: { Upgrade: 'websocket' } })).status).toBe(400);
    const host = await socket(r, 'host');
    expect((await upgrade(r, 'host')).status).toBe(409);
    const peer = await socket(r, 'client');
    const received = message(peer); host.send(new Uint8Array([1, 2, 3])); expect(new Uint8Array(await received)).toEqual(new Uint8Array([1, 2, 3]));
    const returned = message(host); peer.send(new Uint8Array([4, 5])); expect(new Uint8Array(await returned)).toEqual(new Uint8Array([4, 5]));
    const ended = closed(peer); host.close(); await ended;
    expect((await upgrade(r, 'host')).status).toBe(404);
  });
  it('persists a pre-peer hello and maximum-size record as bounded chunks', async () => {
    const r = await create(); const host = await socket(r, 'host');
    host.send(new Uint8Array(150000).fill(23));
    // A DO call acts as an event barrier after the WebSocket delivery.
    await runInDurableObject(stub(), async (_instance, state) => {
      const chunks = await state.storage.list<ArrayBuffer>({ prefix: `queue:${r.room_id}:` });
      expect(chunks.size).toBe(3); for (const value of chunks.values()) expect(value.byteLength).toBeLessThanOrEqual(60000);
    });
    const response = await upgrade(r, 'client'); const peer = response.webSocket!;
    const received = message(peer); peer.accept();
    expect(new Uint8Array(await received)).toEqual(new Uint8Array(150000).fill(23));
    host.close(); peer.close();
  });
  it('runtime-delivered oversized and text records consume both sockets', async () => {
    for (const payload of [new ArrayBuffer(150001), 'text is forbidden']) {
      const r = await create(); const host = await socket(r, 'host'); const peer = await socket(r, 'client');
      const hostEnd = closed(host); const peerEnd = closed(peer); host.send(payload);
      await Promise.all([hostEnd, peerEnd]); expect((await upgrade(r, 'host')).status).toBe(404);
    }
  });
  it('closes on pre-peer queue overflow', async () => {
    const r = await create(); const host = await socket(r, 'host'); const ended = closed(host);
    for (let i = 0; i < 9; i++) host.send(new Uint8Array([i]));
    await ended; expect((await upgrade(r, 'client')).status).toBe(404);
  });
  it('bounds cumulative native send buffering with a nonreplenishing session quota', async () => {
    const r = await create(); const host = await socket(r, 'host'); const peer = await socket(r, 'client');
    for (let i = 0; i < 2; i++) { const received = message(peer); host.send(new ArrayBuffer(150000)); expect((await received).byteLength).toBe(150000); }
    const hostEnd = closed(host); const peerEnd = closed(peer); host.send(new ArrayBuffer(1));
    await Promise.all([hostEnd, peerEnd]); expect((await upgrade(r, 'host')).status).toBe(404);
  });
  it('timer alarm reclaims expired rooms without a client request', async () => {
    const r = await create(); const host = await socket(r, 'host'); const ended = closed(host);
    await runInDurableObject(stub(), async (instance: RelayRooms, state) => {
      const room = (instance as unknown as { rooms: Map<string, { expires: number }> }).rooms.get(r.room_id)!;
      room.expires = Date.now() - 1;
      await state.storage.setAlarm(Date.now() - 1);
    });
    await runDurableObjectAlarm(stub()); await ended;
    expect((await upgrade(r, 'host')).status).toBe(404);
  });
  it('restores attachment state after hibernation and fails closed on interrupted crash state', async () => {
    const r = await create(); const host = await socket(r, 'host'); const peer = await socket(r, 'client');
    await runInDurableObject(stub(), async (_instance, state) => {
      const attachments = state.getWebSockets(r.room_id).map(s => s.deserializeAttachment());
      expect(attachments).toHaveLength(2); expect(attachments.every(a => a.version === 1 && a.room === r.room_id)).toBe(true);
      // Constructor simulation exercises the same persisted-state reconciliation as
      // real isolate hibernation; it does not claim an account-level crash test.
      new RelayRooms(state, env);
      await state.blockConcurrencyWhile(async () => {});
      expect(await state.storage.get(`room:${r.room_id}`)).toBeDefined();
    });
    const hostEnd = closed(host); const peerEnd = closed(peer);
    await runInDurableObject(stub(), async (_instance, state) => {
      const saved = await state.storage.get<Record<string, unknown>>(`room:${r.room_id}`);
      await state.storage.put(`room:${r.room_id}`, { ...saved, busy: true });
      const restored = new RelayRooms(state, env);
      await state.blockConcurrencyWhile(async () => {});
      expect(await state.storage.get(`room:${r.room_id}`)).toBeUndefined();
      const response = await restored.fetch(new Request(`${base}/v1/rooms/${r.room_id}/ws`, { headers: { Upgrade: 'websocket', 'Sec-WebSocket-Protocol': `lightremote.v1, host.${r.host_token}` } }));
      expect(response.status).toBe(404);
    });
    await Promise.all([hostEnd, peerEnd]);
  });
});
