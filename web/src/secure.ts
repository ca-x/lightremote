/** Draft lightremote.v1 PSK bearer transport. No Node-specific dependencies. */
export const MAX_WIRE = 150_000;
export const MAX_RECORDS = 1n << 32n;
export const MAX_KEY_BYTES = 64n * 1024n ** 3n;
const utf8 = new TextEncoder();
const fatal = new TextDecoder("utf-8", { fatal: true });
export type Role = "client" | "host";
export interface Hello {
  type: "lightremote.hello";
  version: 1;
  role: Role;
  nonce: string;
}
export interface Invite {
  relay: string;
  room_id: string;
  client_token: string;
  secret: string;
}
export function base64url(bytes: Uint8Array): string {
  let text = "";
  for (const byte of bytes) text += String.fromCharCode(byte);
  return btoa(text).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}
export function unbase64url(text: string): Uint8Array<ArrayBuffer> {
  if (!/^[A-Za-z0-9_-]+$/.test(text) || text.length % 4 === 1)
    throw new Error("Invalid base64url");
  const bytes = Uint8Array.from(
    atob(text.replace(/-/g, "+").replace(/_/g, "/")),
    (c) => c.charCodeAt(0),
  );
  if (base64url(bytes) !== text) throw new Error("Non-canonical base64url");
  return bytes;
}
export function parseInvite(text: string): Invite {
  const fragment = text.trim().split("#").at(-1)!;
  if (fragment.length > 4096) throw new Error("Invitation is too long");
  let value: Invite;
  try {
    value = JSON.parse(fatal.decode(unbase64url(fragment)));
  } catch {
    throw new Error("Paste a complete invitation link");
  }
  if (
    !value ||
    typeof value !== "object" ||
    Object.keys(value).sort().join(",") !== "client_token,relay,room_id,secret"
  )
    throw new Error("Invalid invitation fields");
  if (
    typeof value.relay !== "string" ||
    typeof value.secret !== "string" ||
    ![value.room_id, value.client_token].every(
      (x) => typeof x === "string" && /^[A-Za-z0-9_-]{1,128}$/.test(x),
    )
  )
    throw new Error("Invalid invitation");
  const url = new URL(value.relay);
  const local = ["localhost", "127.0.0.1", "[::1]"].includes(url.hostname);
  if (
    !(
      ["https:", "wss:"].includes(url.protocol) ||
      (local && ["http:", "ws:"].includes(url.protocol))
    ) ||
    url.username ||
    url.password ||
    url.search ||
    url.hash
  )
    throw new Error(
      "Relay requires TLS outside localhost, without URL credentials",
    );
  if (unbase64url(value.secret).length !== 32)
    throw new Error("Invitation secret must be 32 bytes");
  return value;
}
export function websocketURL(invite: Invite): string {
  const url = new URL(invite.relay);
  url.protocol = ["https:", "wss:"].includes(url.protocol) ? "wss:" : "ws:";
  url.pathname = `${url.pathname.replace(/\/+$/, "")}/v1/rooms/${invite.room_id}/ws`;
  return url.href;
}
export function makeHello(role: Role): Hello {
  return {
    type: "lightremote.hello",
    version: 1,
    role,
    nonce: base64url(crypto.getRandomValues(new Uint8Array(32))),
  };
}
export function parseHello(bytes: Uint8Array, expected: Role): Hello {
  if (bytes.length > 1024) throw new Error("Handshake too large");
  const value = JSON.parse(fatal.decode(bytes));
  if (
    !value ||
    Object.keys(value).sort().join(",") !== "nonce,role,type,version" ||
    value.type !== "lightremote.hello" ||
    value.version !== 1 ||
    value.role !== expected ||
    typeof value.nonce !== "string" ||
    unbase64url(value.nonce).length !== 32
  )
    throw new Error("Invalid peer handshake");
  return value;
}
function owned(bytes: Uint8Array): Uint8Array<ArrayBuffer> {
  return Uint8Array.from(bytes);
}
export class Cipher {
  private next = 0n;
  private totalPlaintext = 0n;
  private busy = false;
  private constructor(
    private key: CryptoKey | undefined,
    private aad: Uint8Array<ArrayBuffer>,
    private role: Role,
  ) {}
  static async derive(
    secret: Uint8Array,
    room: string,
    client: Hello,
    host: Hello,
    role: Role,
  ): Promise<Cipher> {
    if (
      secret.length !== 32 ||
      client.role !== "client" ||
      host.role !== "host"
    )
      throw new Error("Invalid key or roles");
    const c = unbase64url(client.nonce),
      h = unbase64url(host.nonce);
    if (c.length !== 32 || h.length !== 32) throw new Error("Invalid nonce");
    const salt = new Uint8Array(64);
    salt.set(c);
    salt.set(h, 32);
    const aad = utf8.encode(`lightremote/v1/${room}/${role}`);
    const material = await crypto.subtle.importKey(
      "raw",
      owned(secret),
      "HKDF",
      false,
      ["deriveKey"],
    );
    const key = await crypto.subtle.deriveKey(
      { name: "HKDF", hash: "SHA-256", salt, info: aad },
      material,
      { name: "AES-GCM", length: 256 },
      false,
      ["encrypt", "decrypt"],
    );
    salt.fill(0);
    return new Cipher(key, aad, role);
  }
  destroy(): void {
    this.key = undefined;
    this.aad.fill(0);
  }
  private params(): AesGcmParams {
    const iv = new Uint8Array(12),
      view = new DataView(iv.buffer);
    view.setUint32(0, this.role === "client" ? 1 : 2);
    view.setBigUint64(4, this.next);
    return { name: "AES-GCM", iv, additionalData: this.aad, tagLength: 128 };
  }
  private enter(plainLength: number): CryptoKey {
    if (!this.key || this.busy)
      throw new Error("Cipher closed or used concurrently");
    if (
      this.next >= MAX_RECORDS ||
      this.totalPlaintext + BigInt(plainLength) > MAX_KEY_BYTES
    ) {
      this.destroy();
      throw new Error(
        "Cipher per-key usage limit exceeded; create a fresh session",
      );
    }
    this.busy = true;
    return this.key;
  }
  async seal(plain: Uint8Array): Promise<Uint8Array<ArrayBuffer>> {
    if (plain.length > MAX_WIRE - 24) throw new Error("Record too large");
    const key = this.enter(plain.length);
    try {
      const ciphertext = await crypto.subtle.encrypt(
        this.params(),
        key,
        owned(plain),
      );
      if (!this.key) throw new Error("Cipher closed");
      const record = new Uint8Array(8 + ciphertext.byteLength);
      new DataView(record.buffer).setBigUint64(0, this.next++);
      record.set(new Uint8Array(ciphertext), 8);
      this.totalPlaintext += BigInt(plain.length);
      return record;
    } finally {
      this.busy = false;
    }
  }
  async open(record: Uint8Array): Promise<Uint8Array<ArrayBuffer>> {
    if (record.length < 24 || record.length > MAX_WIRE)
      throw new Error("Invalid record length");
    const key = this.enter(record.length - 24);
    try {
      if (
        new DataView(record.buffer, record.byteOffset, 8).getBigUint64(0) !==
        this.next
      )
        throw new Error("Record counter mismatch");
      const plain = await crypto.subtle.decrypt(
        this.params(),
        key,
        owned(record.subarray(8)),
      );
      if (!this.key) throw new Error("Cipher closed");
      this.next++;
      this.totalPlaintext += BigInt(plain.byteLength);
      return new Uint8Array(plain);
    } finally {
      this.busy = false;
    }
  }
}
