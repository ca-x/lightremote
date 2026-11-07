import { test, expect, type Page } from "@playwright/test";
import { readFile } from "node:fs/promises";
import { HostFixture } from "./host-fixture";
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
    expect(fixture.desktopRequests).toEqual([
      { display: 0, fps: 10, quality: 70, single: false },
    ]);
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
    const inputs = fixture.inputs.length,
      resets = fixture.resets;
    await page.getByLabel("Bandwidth", { exact: true }).selectOption("low");
    await expect(page.locator("#bandwidth-help")).toContainText(
      "next time you start a view",
    );
    await expect(page.locator("#desktop-canvas")).toBeVisible();
    await expect(page.locator("#desktop-start")).toBeDisabled();
    expect(fixture.desktopRequests).toHaveLength(1);
    expect(fixture.inputs).toHaveLength(inputs);
    expect(fixture.resets).toBe(resets);
    const incoming = page.waitForEvent("download");
    await page.getByRole("button", { name: "Save screenshot" }).click();
    const item = await incoming;
    expect(await readFile((await item.path())!)).toEqual(
      Buffer.from(fixture.desktopJPEG),
    );
    expect(fixture.desktopRequests.at(-1)).toEqual({
      display: 0,
      fps: 1,
      quality: 90,
      single: true,
    });
    await page.getByRole("button", { name: "Stop", exact: true }).click();
    await expect(page.getByLabel("Enable remote control")).not.toBeChecked();
    await expect(page.locator("#desktop-canvas")).toBeHidden();
    await page.getByRole("button", { name: "Start view", exact: true }).click();
    await expect(page.locator("#desktop-canvas")).toBeVisible();
    expect(fixture.desktopRequests.at(-1)).toEqual({
      display: 0,
      fps: 3,
      quality: 40,
      single: false,
    });
    expect(fixture.desktopRequests).toHaveLength(3);
    await expect(page.getByLabel("Enable remote control")).not.toBeChecked();
    for (const width of [375, 320]) {
      await page.setViewportSize({ width, height: 900 });
      expect(
        await page.evaluate(
          () => document.documentElement.scrollWidth <= innerWidth,
        ),
      ).toBe(true);
    }
    await page.screenshot({
      path: "test-results/desktop-low-bandwidth-mobile.png",
      fullPage: true,
    });
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

test("attended permissions: wait, deny, cancel, explicit grant and revoke without enabling other tools", async ({
  page,
}) => {
  const fixture = await HostFixture.start();
  fixture.setAttended();
  try {
    await connect(page, fixture);
    await expect(page.locator("#clipboard-get")).toBeDisabled();
    await expect(page.locator("#clipboard-set")).toBeDisabled();
    await page.locator("#permission-clipboard_read").check();
    await page
      .getByRole("button", { name: "Request selected permissions" })
      .click();
    await expect(page.locator("#permission-feedback")).toContainText(
      "Waiting for the host",
    );
    await expect(page.locator("#clipboard-get")).toBeDisabled();
    fixture.deny();
    await expect(page.locator("#permission-feedback")).toContainText("denied");
    await expect(page.locator("#permission-clipboard_read")).toBeEnabled();
    await page.locator("#permission-clipboard_read").check();
    await page
      .getByRole("button", { name: "Request selected permissions" })
      .click();
    await expect(
      page.getByRole("button", { name: "Cancel request" }),
    ).toBeEnabled();
    await page.getByRole("button", { name: "Cancel request" }).click();
    await expect(page.locator("#permission-feedback")).toContainText(
      "cancelled",
    );
    expect(fixture.permissions.granted).toEqual([]);
    await page.locator("#permission-clipboard_read").check();
    await page
      .getByRole("button", { name: "Request selected permissions" })
      .click();
    await expect.poll(() => fixture.permissions.pending !== null).toBe(true);
    fixture.approve();
    await expect(page.locator("#permission-feedback")).toContainText(
      "approved",
    );
    await expect(page.locator("#clipboard-get")).toBeEnabled();
    await expect(page.locator("#clipboard-set")).toBeDisabled();
    await page.locator("#clipboard-get").click();
    await expect(page.locator("#clipboard")).toHaveValue("fixture clipboard");
    fixture.revoke(["clipboard_read"]);
    await expect(page.locator("#clipboard-get")).toBeDisabled();
    await expect(page.locator("#clipboard")).toHaveValue("");
    await expect(page.locator("#permission-feedback")).toContainText("revoked");
    await page.getByRole("button", { name: /^Terminal/ }).click();
    await expect(page.locator("#terminal-start")).toBeDisabled();
    expect(
      fixture.calls.filter((c) => c.method === "permissions.request"),
    ).toHaveLength(3);
    await page.setViewportSize({ width: 375, height: 900 });
    await page.getByRole("button", { name: /^Session/ }).click();
    expect(
      await page.evaluate(
        () => document.documentElement.scrollWidth <= innerWidth,
      ),
    ).toBe(true);
    await page.screenshot({
      path: "test-results/permissions-mobile.png",
      fullPage: true,
    });
  } finally {
    await fixture.close();
  }
});

test("revoking shell closes the stream; offline feedback never reuses a consumed invitation", async ({
  page,
  context,
}) => {
  const fixture = await HostFixture.start();
  try {
    await connect(page, fixture);
    await page.getByRole("button", { name: /^Terminal/ }).click();
    await page.locator("#terminal-start").click();
    await expect(page.locator("#terminal-status")).toHaveText("Shell running");
    const resets = fixture.resets;
    fixture.revoke(["shell"]);
    await expect(page.locator("#terminal-start")).toBeDisabled();
    await expect(page.locator("#terminal-status")).toHaveText(
      "No shell running",
    );
    await expect.poll(() => fixture.resets).toBeGreaterThan(resets);
    await context.setOffline(true);
    await expect(page.locator("#network-notice")).toContainText(
      "You are offline",
    );
    await expect(page.locator("#status-label")).toHaveText("Offline");
    await context.setOffline(false);
    await page.getByRole("button", { name: /^Session/ }).click();
    await page.locator("#disconnect").click();
    await page
      .getByLabel("Invitation link")
      .fill(`https://trusted-client.example/#${fixture.fragment()}`);
    await page.locator("#connect").click();
    await expect(page.getByRole("alert")).toContainText("already been used");
  } finally {
    await context.setOffline(false);
    await fixture.close();
  }
});

test("input grant is independent of desktop, and revocation releases control while desktop remains live", async ({
  page,
}) => {
  const fixture = await HostFixture.start();
  try {
    await page.goto("/");
    fixture.desktopJPEG = new Uint8Array(
      Buffer.from(
        await page.evaluate(() => {
          const c = document.createElement("canvas");
          c.width = 16;
          c.height = 12;
          return c.toDataURL("image/jpeg").split(",")[1];
        }),
        "base64",
      ),
    );
    fixture.permissions.granted = fixture.permissions.granted.filter(
      (scope) => scope !== "input",
    );
    await connect(page, fixture, false);
    await page.getByRole("button", { name: /^Desktop/ }).click();
    await page.locator("#desktop-start").click();
    await expect(page.locator("#desktop-canvas")).toBeVisible();
    await expect(page.locator("#enable-input")).toBeDisabled();
    await page.locator("#desktop-canvas").click();
    await page.keyboard.press("a");
    expect(fixture.inputs).toHaveLength(0);
    await page.getByRole("button", { name: /^Session/ }).click();
    await page.locator("#permission-input").check();
    await page.locator("#permissions-request").click();
    await expect.poll(() => !!fixture.permissions.pending).toBe(true);
    fixture.approve();
    await page.getByRole("button", { name: /^Desktop/ }).click();
    await expect(page.locator("#enable-input")).toBeEnabled();
    await page.locator("#enable-input").check();
    await expect(page.locator("#send-text")).toBeEnabled();
    const resets = fixture.resets;
    fixture.revoke(["input"]);
    await expect(page.locator("#enable-input")).not.toBeChecked();
    await expect(page.locator("#enable-input")).toBeDisabled();
    await expect(page.locator("#desktop-canvas")).toBeVisible();
    await expect.poll(() => fixture.resets).toBeGreaterThan(resets);
    const count = fixture.inputs.length;
    await page.locator("#desktop-canvas").click();
    await page.keyboard.press("b");
    expect(fixture.inputs).toHaveLength(count);
  } finally {
    await fixture.close();
  }
});

test("deployed stable update prompts, requires confirmation and never refreshes automatically", async ({
  page,
}) => {
  const fixture = await HostFixture.start();
  let latest = "0.1.1";
  await page.route("**/version.json", (route) =>
    route.fulfill({ json: { version: latest } }),
  );
  try {
    await page.clock.install();
    await connect(page, fixture);
    await expect(page.locator("#update-notice")).toBeHidden();
    latest = "999.0.0";
    await page.clock.fastForward(60_001);
    await expect(page.locator("#update-notice")).toContainText("999.0.0");
    let navigated = false;
    page.on("framenavigated", () => {
      navigated = true;
    });
    page.once("dialog", async (dialog) => {
      expect(dialog.message()).toContain("Disconnect this session");
      await dialog.dismiss();
    });
    await page.locator("#update-refresh").click();
    expect(navigated).toBe(false);
    await expect(page.locator("#status-label")).toHaveText("Connected");
    await expect(
      page.getByRole("link", { name: "Official releases" }),
    ).toHaveAttribute(
      "href",
      "https://github.com/lib-x/lightremote/releases/latest",
    );
  } finally {
    await fixture.close();
  }
});

test("file read revocation aborts a pending streaming save without closing or exporting it", async ({
  page,
}) => {
  const fixture = await HostFixture.start();
  try {
    await page.addInitScript(() => {
      const state = {
        started: false,
        close: 0,
        abort: 0,
        release: undefined as undefined | (() => void),
      };
      (window as any).__permissionSink = state;
      Object.defineProperty(window, "showSaveFilePicker", {
        value: async () => ({
          createWritable: async () => ({
            async write() {
              state.started = true;
              await new Promise<void>((yes) => {
                state.release = yes;
              });
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
      .getByRole("button", { name: "Download hello.txt", exact: true })
      .click();
    await expect
      .poll(() => page.evaluate(() => (window as any).__permissionSink.started))
      .toBe(true);
    fixture.revoke(["file_read"]);
    await expect(page.locator("#files-refresh")).toBeDisabled();
    await expect(
      page.getByRole("button", { name: "Download hello.txt", exact: true }),
    ).toHaveCount(0);
    await page.evaluate(() => (window as any).__permissionSink.release());
    await expect
      .poll(() => page.evaluate(() => (window as any).__permissionSink.abort))
      .toBe(1);
    expect(
      await page.evaluate(() => (window as any).__permissionSink.close),
    ).toBe(0);
    await expect(page.locator("#transfer-status")).toContainText(
      "cancelled or failed",
    );
    await expect(page.locator("#upload")).toBeEnabled();
  } finally {
    await fixture.close();
  }
});

test("heartbeat degradation has a visible weak-network state and recovers after PONG", async ({
  page,
}) => {
  const fixture = await HostFixture.start();
  fixture.holdPongs = true;
  try {
    await page.clock.install();
    await connect(page, fixture);
    await page.clock.fastForward(10_001);
    await expect.poll(() => fixture.pings).toBeGreaterThan(0);
    await page.clock.fastForward(3_001);
    await expect(page.locator("#status-label")).toHaveText("Weak network");
    await expect(page.locator("#network-notice")).toContainText(
      "not retried automatically",
    );
    fixture.holdPongs = false;
    fixture.releasePongs();
    await expect(page.locator("#status")).toHaveAttribute(
      "title",
      /Round-trip/,
    );
    // A delayed PONG still has a high RTT; recovery requires a fresh fast probe.
    await page.clock.fastForward(10_001);
    await expect.poll(() => fixture.pings).toBeGreaterThan(1);
    await expect(page.locator("#status-label")).toHaveText("Connected");
    await expect(page.locator("#network-notice")).toBeHidden();
  } finally {
    await fixture.close();
  }
});
