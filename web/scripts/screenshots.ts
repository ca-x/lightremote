import { chromium, expect } from "@playwright/test";
import { spawn } from "node:child_process";
import { existsSync } from "node:fs";
import { mkdir } from "node:fs/promises";
import { fileURLToPath } from "node:url";

const webRoot = fileURLToPath(new URL("../", import.meta.url));
const output = fileURLToPath(new URL("../../docs/screenshots/", import.meta.url));
const origin = "http://127.0.0.1:4174";
const server = spawn(process.execPath, ["node_modules/vite/bin/vite.js", "preview", "--host", "127.0.0.1", "--port", "4174", "--strictPort"], {
  cwd: webRoot,
  stdio: "pipe",
});
let serverLog = "";
server.stdout.on("data", (chunk) => { serverLog += chunk.toString(); });
server.stderr.on("data", (chunk) => { serverLog += chunk.toString(); });
const executablePath = process.env.CHROMIUM_PATH ||
  ["/opt/google/chrome/chrome", "/usr/bin/chromium"].find(existsSync);

try {
  let ready = false;
  for (let attempt = 0; attempt < 100; attempt++) {
    if (server.exitCode !== null) throw new Error(serverLog);
    await new Promise((resolve) => setTimeout(resolve, 100));
    if (server.exitCode !== null) throw new Error(serverLog);
    if (!serverLog.includes(origin)) continue;
    try {
      ready = (await fetch(origin)).ok;
    } catch {
      ready = false;
    }
    if (ready) break;
  }
  if (!ready) throw new Error(`Preview server did not start: ${serverLog}`);
  await mkdir(output, { recursive: true });
  const browser = await chromium.launch({ executablePath });
  try {
    const desktop = await browser.newContext({ viewport: { width: 1440, height: 1000 }, reducedMotion: "reduce" });
    const page = await desktop.newPage();
    const errors: string[] = [];
    page.on("pageerror", (error) => errors.push(error.message));
    await page.goto(origin);
    await expect(page.getByLabel("Invitation link")).toHaveValue("");
    await expect(page.locator("#status-label")).toHaveText("Disconnected");
    await page.evaluate(() => document.fonts.ready);
    await page.screenshot({ path: `${output}/web-desktop.png`, fullPage: true });
    await page.getByRole("button", { name: /^Desktop/ }).click();
    await page.locator("#desktop-help summary").hover();
    await expect(page.locator("#desktop-help .help-tooltip")).toBeVisible();
    await page.screenshot({ path: `${output}/web-help.png`, fullPage: true });
    await desktop.close();

    const mobile = await browser.newContext({ viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true, reducedMotion: "reduce" });
    const phone = await mobile.newPage();
    phone.on("pageerror", (error) => errors.push(error.message));
    await phone.goto(origin);
    await phone.getByRole("button", { name: /^Terminal/ }).tap();
    await phone.locator("#terminal-help summary").tap();
    await expect(phone.locator("#terminal-help .help-description")).toBeVisible();
    await expect(phone.locator("#terminal-help .help-tooltip")).toBeHidden();
    await phone.getByRole("heading", { name: "Terminal", exact: true }).tap();
    await phone.evaluate(() => window.scrollTo(0, 0));
    await phone.evaluate(() => document.fonts.ready);
    await phone.screenshot({ path: `${output}/web-mobile.png`, fullPage: true });
    expect(errors).toEqual([]);
    await mobile.close();
    console.log(`Saved three production Web screenshots to ${output}`);
  } finally {
    await browser.close();
  }
} finally {
  server.kill("SIGTERM");
}
