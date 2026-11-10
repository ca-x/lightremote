import { test, expect, type Page } from "@playwright/test";
import { readFile } from "node:fs/promises";
import { HostFixture } from "./host-fixture";

test("context help works with pointer, keyboard, Escape and unique decorative icons", async ({ page }) => {
  await page.goto("/");
  await page.getByRole("button", { name: /^Desktop/ }).click();
  const details = page.locator("#desktop-help");
  const trigger = details.locator("summary");
  const tooltip = details.locator(".help-tooltip");
  await trigger.hover();
  await expect(tooltip).toBeVisible();
  await trigger.focus();
  await page.keyboard.press("Escape");
  await expect(tooltip).toBeHidden();
  await page.keyboard.press("Enter");
  await expect(details).toHaveAttribute("open", "");
  await expect(details.locator(".help-description")).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(details).not.toHaveAttribute("open", "");
  await expect(trigger).toBeFocused();
  const ids = await page.locator("svg [id]").evaluateAll((nodes) => nodes.map((node) => node.id));
  expect(new Set(ids).size).toBe(ids.length);
  expect(await page.locator('svg:not([aria-hidden="true"])').count()).toBe(0);
  const license = await page.request.get("/kubai-icons-LICENSE.txt");
  expect(license.ok()).toBe(true);
  expect(await license.text()).toContain("Copyright (c) 2026 Kubai087");
});

test("touch help needs no hover at narrow widths and larger text", async ({ browser }) => {
  const context = await browser.newContext({
    viewport: { width: 320, height: 800 },
    hasTouch: true,
    isMobile: true,
    reducedMotion: "reduce",
  });
  try {
    const page = await context.newPage();
    await page.goto("http://127.0.0.1:4173/");
    await page.getByRole("button", { name: /^Terminal/ }).tap();
    const details = page.locator("#terminal-help");
    const trigger = details.locator("summary");
    await trigger.evaluate((element) => { element.style.fontSize = "20px"; });
    await trigger.tap();
    await expect(details.locator(".help-description")).toContainText("Switching tools keeps the shell open");
    await expect(details.locator(".help-description")).toBeVisible();
    await expect(details.locator(".help-tooltip")).toBeHidden();
    expect((await trigger.boundingBox())!.height).toBeGreaterThanOrEqual(48);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await trigger.tap();
    await expect(details.locator(".help-description")).toBeHidden();
  } finally {
    await context.close();
  }
});
async function connect(page: Page, fixture: HostFixture, hash = true) {
  await page.goto(hash ? `/#${fixture.fragment()}` : "/");
  if (!hash)
    await page
      .getByLabel("Invitation link")
      .fill(`https://trusted-client.example/#${fixture.fragment()}`);
  await page.getByRole("button", { name: /Connect securely/ }).click();
  await expect(page.locator("#status-label")).toHaveText("Connected");
  await expect(page.locator("#capability-status")).toHaveText(
    "Reported by host",
  );
}
test("production browser boot, validation, and responsive workstation without Node polyfills", async ({
  page,
}) => {
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.goto("/");
  await expect(page).toHaveTitle("roc-remote · Workspace");
  const brand = page.getByRole("link", { name: "roc-remote home" });
  await expect(brand).toBeVisible();
  await expect
    .poll(() => brand.locator("img").evaluate((image: HTMLImageElement) => image.naturalWidth))
    .toBe(256);
  await expect(
    page.getByRole("heading", { name: "Session", exact: true }),
  ).toBeVisible();
  await page.screenshot({
    path: "test-results/workstation-desktop.png",
    fullPage: true,
  });
  await page.getByLabel("Invitation link").fill("invalid");
  await page.getByRole("button", { name: /Connect securely/ }).click();
  await expect(page.getByRole("alert")).toContainText(
    "Paste a complete invitation link",
  );
  await page.getByRole("button", { name: "Dismiss error" }).click();
  for (const width of [375, 320]) {
    await page.setViewportSize({ width, height: 900 });
    for (const view of ["Session", "Desktop", "Terminal", "Files"]) {
      await page
        .getByRole("button", { name: new RegExp(`^${view}`) })
        .first()
        .click();
      expect(
        await page.evaluate(
          () => document.documentElement.scrollWidth <= innerWidth,
        ),
      ).toBe(true);
    }
  }
  await page.getByRole("button", { name: /^Session/ }).click();
  await page.getByLabel("Invitation link").fill("");
  await page.screenshot({
    path: "test-results/workstation-mobile.png",
    fullPage: true,
  });
  expect(errors).toEqual([]);
});
test("encrypted actual SDK negotiation, explicit clipboard, xterm, WINDOW resumes, upload and FIN download", async ({
  page,
}) => {
  const fixture = await HostFixture.start();
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.addInitScript(() => {
    Object.defineProperty(window, "showSaveFilePicker", {
      value: undefined,
      configurable: true,
    });
  });
  try {
    await connect(page, fixture);
    expect(new URL(page.url()).hash).toBe("");
    expect(fixture.calls.some((c) => c.method.startsWith("clipboard."))).toBe(
      false,
    );
    await page.getByRole("button", { name: "Read from host" }).click();
    await expect(page.getByLabel("Clipboard text")).toHaveValue(
      "fixture clipboard",
    );
    await page.getByLabel("Clipboard text").fill("explicit clipboard");
    await page.getByRole("button", { name: "Send to host" }).click();
    await expect.poll(() => fixture.clipboard).toBe("explicit clipboard");
    await page.getByRole("button", { name: /^Terminal/ }).click();
    await page.getByRole("button", { name: "Open terminal" }).click();
    await expect(page.locator("#terminal-status")).toHaveText("Shell running");
    await page.keyboard.type("echo browser smoke");
    await page.keyboard.press("Enter");
    await expect
      .poll(() => fixture.shellInput)
      .toContain("echo browser smoke\r");
    await page.setViewportSize({ width: 1120, height: 900 });
    await expect
      .poll(
        () => fixture.calls.filter((c) => c.method === "shell.resize").length,
      )
      .toBeGreaterThan(0);
    await page.getByRole("button", { name: "Close terminal" }).click();
    await page.getByRole("button", { name: /^Files/ }).click();
    await expect(
      page.getByRole("button", { name: "Download large.bin", exact: true }),
    ).toBeVisible();
    const incoming = page.waitForEvent("download");
    await page
      .getByRole("button", { name: "Download large.bin", exact: true })
      .click();
    const item = await incoming;
    const location = await item.path();
    expect(location).not.toBeNull();
    expect(await readFile(location!)).toEqual(
      Buffer.from(fixture.files.get("large.bin")!),
    );
    const uploadBytes = Buffer.alloc(500000);
    for (let i = 0; i < uploadBytes.length; i++) uploadBytes[i] = i % 251;
    await page.locator("#upload").setInputFiles({
      name: "browser-upload.bin",
      mimeType: "application/octet-stream",
      buffer: uploadBytes,
    });
    await expect(page.locator("#transfer-status")).toContainText("Uploaded");
    expect(Buffer.from(fixture.files.get("browser-upload.bin")!)).toEqual(
      uploadBytes,
    );
    expect(fixture.windows).toBeGreaterThan(20);
    expect(fixture.remoteFins).toBeGreaterThanOrEqual(2);
    await page.getByRole("button", { name: /^Desktop/ }).click();
    await page.getByRole("button", { name: "Start view", exact: true }).click();
    await expect(page.getByRole("alert")).toContainText(
      "Desktop capture unavailable",
    );
    await expect(page.locator("#desktop-canvas")).toBeHidden();
    await page.getByRole("button", { name: /^Session/ }).click();
    await page.getByRole("button", { name: "Disconnect", exact: true }).click();
    await expect(page.locator("#status-label")).toHaveText("Disconnected");
    expect(errors).toEqual([]);
    expect(fixture.errors).toEqual([]);
  } finally {
    await fixture.close();
  }
});
test("JPEG fixture rendering, single screenshot and pointer/key/text input through encrypted streams", async ({
  page,
}) => {
  const fixture = await HostFixture.start();
  try {
    await page.goto("/");
    const jpeg = await page.evaluate(() => {
      const canvas = document.createElement("canvas");
      canvas.width = 160;
      canvas.height = 100;
      const ctx = canvas.getContext("2d")!;
      ctx.fillStyle = "#278b76";
      ctx.fillRect(0, 0, 160, 100);
      return canvas.toDataURL("image/jpeg").split(",")[1];
    });
    fixture.desktopJPEG = new Uint8Array(Buffer.from(jpeg, "base64"));
    await connect(page, fixture, false);
    await page.getByRole("button", { name: /^Desktop/ }).click();
    await page.getByRole("button", { name: "Find displays" }).click();
    await expect(page.getByLabel("Display", { exact: true })).toContainText(
      "Fixture display",
    );
    await page.getByRole("button", { name: "Start view", exact: true }).click();
    await expect(page.locator("#desktop-canvas")).toBeVisible();
    await expect(page.locator("#desktop-status")).toContainText("160 × 100");
    await page.getByLabel("Enable remote control").check();
    await expect.poll(() => page.locator("#send-text").isEnabled()).toBe(true);
    await page.locator("#desktop-canvas").click({ position: { x: 30, y: 20 } });
    await page.keyboard.press("a");
    await page.keyboard.down("Shift");
    await page.keyboard.down("KeyA");
    await page.keyboard.up("Shift");
    await page.keyboard.up("KeyA");
    await expect
      .poll(() =>
        fixture.inputs.some(
          (e: any) => e.type === "key" && e.key === "A" && !e.down,
        ),
      )
      .toBe(true);
    await page.locator("#remote-text").fill("你好");
    await page.getByRole("button", { name: "Send text", exact: true }).click();
    await expect
      .poll(() =>
        fixture.inputs.some((e: any) => e.type === "text" && e.text === "你好"),
      )
      .toBe(true);
    expect(fixture.inputs.some((e: any) => e.type === "button" && e.down)).toBe(
      true,
    );
    expect(
      fixture.inputs.some((e: any) => e.type === "key" && e.key === "a"),
    ).toBe(true);
    const incoming = page.waitForEvent("download");
    await page.getByRole("button", { name: "Save screenshot" }).click();
    const item = await incoming;
    expect(await readFile((await item.path())!)).toEqual(
      Buffer.from(fixture.desktopJPEG),
    );
    await page.getByRole("button", { name: "Stop", exact: true }).click();
    await expect(page.getByLabel("Enable remote control")).not.toBeChecked();
    await expect(page.locator("#desktop-canvas")).toBeHidden();
    expect(fixture.errors).toEqual([]);
  } finally {
    await fixture.close();
  }
});

test("production GOAWAY drains a streaming file save and control FIN before Pong-confirmed carrier close", async ({
  page,
}) => {
  const fixture = await HostFixture.start();
  fixture.holdPongs = true;
  try {
    await page.addInitScript(() => {
      const state = {
        chunks: [] as number[][],
        started: false,
        close: 0,
        abort: 0,
        release: undefined as undefined | (() => void),
      };
      (window as any).__drainSink = state;
      Object.defineProperty(window, "showSaveFilePicker", {
        value: async () => ({
          createWritable: async () => ({
            async write(bytes: Uint8Array) {
              state.chunks.push(Array.from(bytes));
              if (!state.started) {
                state.started = true;
                await new Promise<void>((resolve) => {
                  state.release = resolve;
                });
              }
            },
            async close() {
              state.close++;
            },
            async abort() {
              state.abort++;
            },
          }),
        }),
      });
    });
    await connect(page, fixture);
    await page.getByRole("button", { name: /^Files/ }).click();
    await page
      .getByRole("button", { name: "Download large.bin", exact: true })
      .click();
    await expect
      .poll(() => page.evaluate(() => (window as any).__drainSink.started))
      .toBe(true);
    fixture.goaway();
    fixture.endControl();
    await expect.poll(() => fixture.remoteFins).toBeGreaterThanOrEqual(2);
    await expect(page.locator("#status-label")).toHaveText("Connected");
    await page.evaluate(() => (window as any).__drainSink.release());
    await expect(page.locator("#transfer-status")).toHaveText(
      "Downloaded large.bin",
    );
    const state = await page.evaluate(() => {
      const state = (window as any).__drainSink;
      return {
        bytes: state.chunks.flat(),
        close: state.close,
        abort: state.abort,
      };
    });
    expect(Buffer.from(state.bytes)).toEqual(
      Buffer.from(fixture.files.get("large.bin")!),
    );
    expect(state.close).toBe(1);
    expect(state.abort).toBe(0);
    await expect.poll(() => fixture.pings).toBe(1);
    await expect(page.locator("#status-label")).toHaveText("Connected");
    fixture.releasePongs();
    await expect(page.locator("#status-label")).toHaveText("Disconnected");
    expect(fixture.errors).toEqual([]);
  } finally {
    await fixture.close();
  }
});

test("oversized JPEG headers are rejected before desktop decode or screenshot export", async ({
  page,
}) => {
  const fixture = await HostFixture.start();
  try {
    // No compressed raster, and a decoder stub ensures this test can never allocate a bomb.
    fixture.desktopJPEG = Uint8Array.from([
      0xff, 0xd8, 0xff, 0xc0, 0, 11, 8, 0x40, 0, 0x40, 0, 1, 1, 0x11, 0, 0xff,
      0xda, 0, 8, 1, 1, 0, 0, 63, 0, 0xff, 0xd9,
    ]);
    await page.addInitScript(() => {
      (window as any).__jpegDecodeCalls = 0;
      window.createImageBitmap = (async () => {
        (window as any).__jpegDecodeCalls++;
        throw new Error("Unsafe decode was attempted");
      }) as typeof createImageBitmap;
    });
    let exports = 0;
    page.on("download", () => {
      exports++;
    });
    await connect(page, fixture);
    await page.getByRole("button", { name: /^Desktop/ }).click();
    await page.getByRole("button", { name: "Start view", exact: true }).click();
    await expect(page.getByRole("alert")).toContainText(
      "JPEG dimensions exceed",
    );
    await expect(page.locator("#desktop-canvas")).toBeHidden();
    expect(await page.evaluate(() => (window as any).__jpegDecodeCalls)).toBe(
      0,
    );
    await page.getByRole("button", { name: "Dismiss error" }).click();
    await page.getByRole("button", { name: "Save screenshot" }).click();
    await expect(page.getByRole("alert")).toContainText(
      "JPEG dimensions exceed",
    );
    expect(exports).toBe(0);
    expect(await page.evaluate(() => (window as any).__jpegDecodeCalls)).toBe(
      0,
    );
  } finally {
    await fixture.close();
  }
});
