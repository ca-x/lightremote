import { createHash } from "node:crypto";
const url =
  "https://github.com/lib-x/tm/releases/download/sdk%2Fnode%2Fv0.1.0/tunnel-mux-node-0.1.0.tgz";
const expected =
  "ff3d5931781beeeb050de88f9fe3033f4810d6a6d122e399505bbca8f7be3cf9";
const response = await fetch(url);
if (!response.ok) throw new Error(`SDK download failed (${response.status})`);
const bytes = new Uint8Array(await response.arrayBuffer());
if (bytes.length !== 37766)
  throw new Error("SDK asset size differs from verified release");
const actual = createHash("sha256").update(bytes).digest("hex");
if (actual !== expected) throw new Error("SDK release digest mismatch");
console.log(
  `Verified @lib-x/tmp 0.1.0 (${bytes.length} bytes), SHA-256 ${actual}`,
);
