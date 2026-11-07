/** Only stable release versions may produce a refresh prompt. */
const stableVersion =
  /^v?(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$/;
export function isNewerVersion(candidate: string, current: string): boolean {
  const parse = (value: string) => stableVersion.exec(value);
  if (candidate.length > 128 || current.length > 128) return false;
  const next = parse(candidate),
    base = parse(current);
  if (!next || !base) return false;
  for (let i = 1; i <= 3; i++) {
    const a = BigInt(next[i]),
      b = BigInt(base[i]);
    if (a !== b) return a > b;
  }
  return false;
}
export function versionOf(value: unknown): string | undefined {
  if (!value || typeof value !== "object") return;
  const version = (value as { version?: unknown }).version;
  if (
    typeof version === "string" &&
    version.length <= 128 &&
    stableVersion.test(version)
  )
    return version;
}
export function watchUpdates(options: {
  currentVersion?: string;
  onUpdate(version: string): void;
  fetch?: typeof fetch;
  online?: () => boolean;
  intervalMs?: number;
  timeoutMs?: number;
}) {
  let current = options.currentVersion,
    stopped = false,
    active: AbortController | undefined;
  let announced = current;
  const request = options.fetch ?? fetch;
  async function check() {
    if (stopped || active || !(options.online?.() ?? true)) return;
    const abort = new AbortController();
    active = abort;
    const timer = setTimeout(() => abort.abort(), options.timeoutMs ?? 5000);
    try {
      const response = await request(
        `${import.meta.env?.BASE_URL ?? "/"}version.json`,
        {
          cache: "no-store",
          signal: abort.signal,
          credentials: "same-origin",
          redirect: "error",
        },
      );
      if (!response.ok) return;
      if (
        Number(response.headers.get("content-length")) > 4096 ||
        !response.body
      )
        return;
      const reader = response.body.getReader();
      const chunks: Uint8Array[] = [];
      let size = 0;
      try {
        for (;;) {
          const { done, value } = await reader.read();
          if (done) break;
          size += value.length;
          if (size > 4096 || abort.signal.aborted) {
            await reader.cancel();
            return;
          }
          chunks.push(value);
        }
      } finally {
        reader.releaseLock();
      }
      const bytes = new Uint8Array(size);
      let offset = 0;
      for (const chunk of chunks) {
        bytes.set(chunk, offset);
        offset += chunk.length;
      }
      const version = versionOf(JSON.parse(new TextDecoder().decode(bytes)));
      if (
        stopped ||
        abort.signal.aborted ||
        !(options.online?.() ?? true) ||
        !version
      )
        return;
      // Without a bundled identifier, the first successful fetch is only a baseline.
      if (!current) {
        current = announced = version;
        return;
      }
      if (
        isNewerVersion(version, current) &&
        (!announced || isNewerVersion(version, announced))
      ) {
        announced = version;
        options.onUpdate(version);
      }
    } catch {
      /* Offline, invalid manifests and timeouts never announce an update. */
    } finally {
      clearTimeout(timer);
      if (active === abort) active = undefined;
    }
  }
  const interval = setInterval(
    () => void check(),
    options.intervalMs ?? 60_000,
  );
  void check();
  return {
    check,
    stop() {
      stopped = true;
      clearInterval(interval);
      active?.abort();
    },
  };
}
