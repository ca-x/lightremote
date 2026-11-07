import { defineConfig } from "vite";
import { readFileSync } from "node:fs";

// One version source for native binaries, browser code and deployed manifests.
// A branch update alone does not imply a new GitHub release or a deployment.
const manifest = readFileSync(new URL("../Cargo.toml", import.meta.url), "utf8");
const version = manifest.match(/\[workspace\.package\][\s\S]*?^version\s*=\s*"([^"]+)"/m)?.[1];
if (!version || !/^\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?$/.test(version)) {
  throw new Error("Missing workspace release version");
}
const versionJSON = JSON.stringify({ version });

export default defineConfig({
  build: { target: "es2022" },
  define: { __APP_VERSION__: JSON.stringify(version) },
  plugins: [{
    name: "lightremote-deployment-version",
    generateBundle() {
      this.emitFile({ type: "asset", fileName: "version.json", source: versionJSON });
    },
    configureServer(server) {
      server.middlewares.use("/version.json", (_request, response) => {
        response.setHeader("Content-Type", "application/json");
        response.setHeader("Cache-Control", "no-store");
        response.end(versionJSON);
      });
    },
  }],
});
