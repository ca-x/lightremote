/** Optional real Rust relay + agent acceptance. Requires prebuilt binaries; never builds siblings. */
import { chromium, expect } from "@playwright/test";
import { spawn, type ChildProcess } from "node:child_process";
import { existsSync } from "node:fs";
import { mkdir, mkdtemp, readFile, writeFile, rm } from "node:fs/promises";
import { createServer } from "node:net";
import { randomBytes } from "node:crypto";
import { resolve } from "node:path";
const cli = process.env.ROC_REMOTE_BIN,
  relayBin = process.env.ROC_REMOTE_RELAY_BIN;
if (!cli || !relayBin)
  throw new Error(
    "Set ROC_REMOTE_BIN and ROC_REMOTE_RELAY_BIN to prebuilt Rust binaries",
  );
const artifacts = resolve("test-results");
await mkdir(artifacts, { recursive: true });
const share = await mkdtemp(resolve(artifacts, "rust-share-"));
const source = Buffer.alloc(700000);
for (let i = 0; i < source.length; i++) source[i] = i % 251;
await writeFile(resolve(share, "source.bin"), source);
const portServer = createServer();
await new Promise<void>((yes) => portServer.listen(0, "127.0.0.1", yes));
const address = portServer.address();
if (!address || typeof address === "string") throw new Error("No local port");
const port = address.port;
await new Promise<void>((yes) => portServer.close(() => yes()));
const origin = `http://127.0.0.1:${port}`;
const token = randomBytes(32).toString("base64url");
const env = { ...process.env, ROC_REMOTE_RELAY_TOKEN: token };
let relay: ChildProcess | undefined, host: ChildProcess | undefined;
let browser: Awaited<ReturnType<typeof chromium.launch>> | undefined;
async function stop(child?: ChildProcess) {
  if (!child || child.exitCode !== null || child.signalCode !== null) return;
  const ended = new Promise<void>((yes) => child.once("exit", () => yes()));
  child.kill("SIGINT");
  const timer = setTimeout(() => child.kill("SIGKILL"), 2500);
  await ended;
  clearTimeout(timer);
}
try {
  relay = spawn(relayBin, [], {
    env: {
      ...env,
      ROC_REMOTE_BIND: `127.0.0.1:${port}`,
      ROC_REMOTE_ALLOWED_ORIGINS: origin,
      ROC_REMOTE_WEB_DIR: resolve("dist"),
    },
    stdio: ["ignore", "ignore", "pipe"],
  });
  for (let tries = 0; ; tries++) {
    try {
      if ((await fetch(`${origin}/healthz`)).ok) break;
    } catch {}
    if (tries > 100 || relay.exitCode !== null)
      throw new Error("Rust relay failed to start");
    await new Promise((yes) => setTimeout(yes, 50));
  }
  host = spawn(
    cli,
    [
      "host",
      "--relay",
      origin,
      "--web-url",
      origin,
      "--share-dir",
      share,
      "--allow-shell",
      "--allow-write",
      "--shell",
      "/bin/sh",
    ],
    { env, stdio: ["ignore", "pipe", "pipe"] },
  );
  // Do not print the invitation or copy it to test artifacts.
  const invitation = await new Promise<string>((yes, no) => {
    let bytes = "";
    const timer = setTimeout(
      () => no(new Error("Host invitation timeout")),
      10000,
    );
    host!.stdout!.on("data", (chunk) => {
      bytes += chunk.toString();
      if (bytes.length > 8192) {
        clearTimeout(timer);
        no(new Error("Host output limit"));
        return;
      }
      const match = bytes.match(/^INVITE (.+)$/m);
      if (match) {
        clearTimeout(timer);
        yes(match[1].trim());
      }
    });
    host!.once("exit", () => {
      clearTimeout(timer);
      no(new Error("Host exited before invitation"));
    });
  });
  const executablePath =
    process.env.CHROMIUM_PATH ||
    (existsSync("/opt/google/chrome/chrome")
      ? "/opt/google/chrome/chrome"
      : existsSync("/usr/bin/chromium")
        ? "/usr/bin/chromium"
        : undefined);
  browser = await chromium.launch({ executablePath });
  const page = await browser.newPage({
    viewport: { width: 1440, height: 1000 },
    acceptDownloads: true,
  });
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.addInitScript(() =>
    Object.defineProperty(window, "showSaveFilePicker", { value: undefined }),
  );
  await page.goto(invitation);
  await page.getByRole("button", { name: /Connect securely/ }).click();
  await expect(page.locator("#status-label")).toHaveText("Connected");
  await expect(page.locator("#capability-status")).toHaveText(
    "Reported by host",
  );
  await page.getByRole("button", { name: /^Terminal/ }).click();
  await page.getByRole("button", { name: "Open terminal" }).click();
  await expect(page.locator("#terminal-status")).toHaveText("Shell running");
  await page.keyboard.type(
    "printf '\\114\\122\\137\\120\\124\\131\\137\\117\\113\\n'",
  );
  await page.keyboard.press("Enter");
  await expect(page.locator("#terminal")).toContainText("LR_PTY_OK");
  const oldDimensions = await page.locator("#terminal-size").textContent();
  await page.setViewportSize({ width: 1000, height: 800 });
  await expect(page.locator("#terminal-size")).not.toHaveText(oldDimensions!);
  const [cols, rows] = (await page
    .locator("#terminal-size")
    .textContent())!.split(" × ");
  // Validate host-applied dimensions through the actual PTY, without trusting UI alone.
  await expect
    .poll(async () => {
    await page.keyboard.type("stty size");
      await page.keyboard.press("Enter");
      return (await page.locator("#terminal").textContent()) || "";
    })
    .toContain(`${rows} ${cols}`);
  await page.getByRole("button", { name: "Close terminal" }).click();
  await page.getByRole("button", { name: /^Files/ }).click();
  await expect(
    page.getByRole("button", { name: "Download source.bin", exact: true }),
  ).toBeVisible();
  const downloadEvent = page.waitForEvent("download");
  await page
    .getByRole("button", { name: "Download source.bin", exact: true })
    .click();
  const downloaded = await downloadEvent;
  expect(await readFile((await downloaded.path())!)).toEqual(source);
  const uploaded = Buffer.alloc(500000, 83);
  await page
    .locator("#upload")
    .setInputFiles({
      name: "uploaded.bin",
      mimeType: "application/octet-stream",
      buffer: uploaded,
    });
  await expect(page.locator("#transfer-status")).toContainText("Uploaded");
  expect(await readFile(resolve(share, "uploaded.bin"))).toEqual(uploaded);
  await page.getByRole("button", { name: /^Desktop/ }).click();
  await page.getByRole("button", { name: "Start view", exact: true }).click();
  await expect(page.getByRole("alert")).toBeVisible();
  await expect(page.locator("#desktop-canvas")).toBeHidden();
  await page.screenshot({
    path: resolve(artifacts, "rust-headless-desktop.png"),
  });
  await page.getByRole("button", { name: /^Session/ }).click();
  await page.getByRole("button", { name: "Disconnect", exact: true }).click();
  await expect(page.locator("#status-label")).toHaveText("Disconnected");
  expect(errors).toEqual([]);
  console.log(
    "PASS: real Chromium → Rust relay → encrypted TMP Rust host: PTY output, resize, upload, download, and truthful headless desktop error.",
  );
} finally {
  await browser?.close();
  await stop(host);
  await stop(relay);
  await rm(share, { recursive: true, force: true });
}
