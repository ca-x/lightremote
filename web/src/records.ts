/** One bounded partial application record; processes coalesced records without retaining them. */
export class Records {
  private header = new Uint8Array(4);
  private headerUsed = 0;
  private body?: Uint8Array;
  private used = 0;
  constructor(readonly limit = 8 * 1024 * 1024) {}
  async feed(
    chunk: Uint8Array,
    handle: (record: Uint8Array) => void | Promise<void>,
  ): Promise<void> {
    let offset = 0;
    while (offset < chunk.length) {
      if (!this.body) {
        const n = Math.min(4 - this.headerUsed, chunk.length - offset);
        this.header.set(chunk.subarray(offset, offset + n), this.headerUsed);
        this.headerUsed += n;
        offset += n;
        if (this.headerUsed < 4) continue;
        const length = new DataView(this.header.buffer).getUint32(0);
        if (length > this.limit)
          throw new Error("Application record exceeds limit");
        this.body = new Uint8Array(length);
        this.used = 0;
      }
      const n = Math.min(this.body.length - this.used, chunk.length - offset);
      this.body.set(chunk.subarray(offset, offset + n), this.used);
      this.used += n;
      offset += n;
      if (this.used === this.body.length) {
        const record = this.body;
        this.body = undefined;
        this.headerUsed = 0;
        this.used = 0;
        await handle(record);
      }
    }
  }
  finish(): void {
    if (this.headerUsed || this.body)
      throw new Error("Stream ended inside an application record");
  }
}
export function frame(body: Uint8Array): Uint8Array<ArrayBuffer> {
  if (body.length > 8 * 1024 * 1024)
    throw new Error("Application record exceeds limit");
  const result = new Uint8Array(body.length + 4);
  new DataView(result.buffer).setUint32(0, body.length);
  result.set(body, 4);
  return result;
}
export function jsonFrame(value: unknown): Uint8Array<ArrayBuffer> {
  return frame(new TextEncoder().encode(JSON.stringify(value)));
}
export function parseJSON(bytes: Uint8Array): unknown {
  return JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(bytes));
}
