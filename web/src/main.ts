import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";
import "./style.css";
import { Link, type MuxStream } from "./link";
import { parseInvite } from "./secure";
import { Records, jsonFrame } from "./records";
import { jpegDimensions } from "./jpeg";
import { Control, object, upload, download } from "./client";

const icons: Record<string, string> = {
  session:
    '<rect x="3" y="3" width="18" height="18" rx="4"/><path d="M7 8h10M7 12h6M7 16h8"/>',
  desktop:
    '<rect x="2" y="3" width="20" height="14" rx="2"/><path d="M8 21h8M12 17v4"/>',
  terminal: '<path d="m5 7 5 5-5 5m8 0h6"/>',
  files: '<path d="M3 7V4h6l3 3h9v13H3z"/>',
};
const icon = (name: string) =>
  `<svg viewBox="0 0 24 24" width="20" height="20" fill="none" stroke="currentColor" stroke-width="1.6" aria-hidden="true">${icons[name]}</svg>`;
const $ = <T extends HTMLElement = HTMLElement>(id: string) =>
  document.getElementById(id) as T;
const button = (id: string) => $<HTMLButtonElement>(id);
const input = (id: string) => $<HTMLInputElement>(id);
const text = (id: string, value: string) => {
  $(id).textContent = value;
};
$("app").innerHTML = `
<a class="skip" href="#main">Skip to workspace</a>
<aside class="sidebar">
  <a class="brand" href="#" aria-label="LightRemote home"><span class="brand-symbol">lr<span>·</span></span><span>LightRemote<small>REMOTE WORKSPACE</small></span></a>
  <div class="workspace-label">WORKSPACE <span>01</span></div>
  <nav aria-label="Workspace">${["Session", "Desktop", "Terminal", "Files"].map((name, n) => `<button class="nav-item ${n === 0 ? "active" : ""}" data-view="${name.toLowerCase()}" ${n === 0 ? 'aria-current="page"' : ""}>${icon(name.toLowerCase())}<span>${name}</span><span class="nav-index">0${n + 1}</span></button>`).join("")}</nav>
  <div class="sidebar-bottom"><span class="lock-symbol" aria-hidden="true">◇</span><strong>Private by design</strong><p>End-to-end encrypted.<br>Permission stays with the host.</p><span class="version">LIGHTREMOTE / V0.1</span></div>
</aside>
<div class="workspace">
  <header class="topbar"><span><span class="muted">Workspace</span><span class="slash">/</span><span id="breadcrumb">Session</span></span><span class="status" id="status" role="status"><i></i><span id="status-label">Disconnected</span></span></header>
  <main id="main" tabindex="-1">
    <div id="error" class="notice error" role="alert" hidden><span id="error-text"></span><button id="dismiss-error" aria-label="Dismiss error">×</button></div>
    <section id="view-session" class="view">
      <div class="page-heading"><div><p class="eyebrow">CONNECTION</p><h1>Session</h1><p class="lead">Connect to a host and choose a tool.</p></div><span class="section-number">01 / SESSION</span></div>
      <div class="session-grid">
        <section class="connection-panel"><div class="section-title"><h2>Connect to a host</h2><span class="tag">INVITATION ONLY</span></div>
          <p>Paste the invitation link from your host. Keep it private: anyone with the link can join this session.</p>
          <form id="connect-form"><label for="invitation">Invitation link</label><textarea id="invitation" rows="3" placeholder="https://your-client.example/#…" autocomplete="off" spellcheck="false" required aria-describedby="invite-help"></textarea><p class="field-help" id="invite-help">The invitation secret stays in this browser session.</p><div class="actions"><button class="primary" id="connect" type="submit">Connect securely <span aria-hidden="true">↗</span></button><button type="button" id="disconnect" disabled>Disconnect</button></div></form>
          <div class="connection-details"><div><span>Relay</span><strong id="relay-name">Waiting for an invitation</strong></div><div><span>Room</span><strong class="mono" id="room-name">Not connected</strong></div><div><span>Transport</span><strong>WebSocket · encrypted TMP</strong></div></div>
        </section>
        <section class="session-note"><span class="note-symbol" aria-hidden="true">◇</span><h2>Host permissions</h2><p>The host decides which tools are available. Desktop and input need local permission. Shell and files work on headless hosts.</p><div class="rule"></div><p class="small">Use a client origin you trust. For sensitive sessions, serve this client independently of the relay.</p><span class="tag">NO BACKGROUND CLIPBOARD ACCESS</span></section>
      </div>
      <section class="capabilities-section"><div class="section-title"><h2>Host capabilities</h2><span class="muted small" id="capability-status">Available after connection</span></div><div id="capabilities" class="capabilities"><p class="muted">Connect to discover what this host allows.</p></div></section>
      <section class="clipboard-section"><div><p class="eyebrow">ON YOUR TERMS</p><h2>Clipboard</h2><p>Read or change the host clipboard only when you choose.</p></div><div><label for="clipboard">Clipboard text</label><textarea id="clipboard" rows="3" placeholder="Text to share with the host" maxlength="262144"></textarea><div class="actions wrap"><button id="clipboard-get" data-connected disabled>Read from host</button><button id="clipboard-set" data-connected disabled>Send to host</button><button id="clipboard-local">Paste from this device</button><button id="clipboard-copy">Copy to this device</button></div><p class="field-help" id="clipboard-status" role="status">No clipboard access requested.</p></div></section>
    </section>
    <section id="view-desktop" class="view" hidden>
      <div class="page-heading"><div><p class="eyebrow">REMOTE VIEW</p><h1>Desktop</h1><p class="lead">A live view, with control when you enable it.</p></div><span class="section-number">02 / DESKTOP</span></div>
      <div class="toolbar"><label class="inline-label">Display <select id="display" aria-label="Display"><option value="0">Display 0</option></select></label><button id="refresh-displays" data-connected disabled>Find displays</button><div class="toolbar-spacer"></div><button id="desktop-start" class="primary" data-connected disabled>Start view</button><button id="desktop-stop" disabled>Stop</button><button id="screenshot" data-connected disabled>Save screenshot</button></div>
      <div class="desktop-stage" id="desktop-stage"><div class="stage-empty" id="desktop-empty">${icon("desktop")}<h2>Your desktop appears here</h2><p id="desktop-message">Connect to a host, then start a view.</p></div><canvas id="desktop-canvas" tabindex="0" aria-label="Remote desktop. Enable control to send pointer and keyboard input." hidden></canvas></div>
      <div class="desktop-footer"><label class="check"><input id="enable-input" type="checkbox" disabled> Enable remote control</label><span id="desktop-status" role="status">No desktop stream</span><span class="muted small">JPEG / encrypted TMP</span></div>
      <div class="text-input-row"><label class="sr-only" for="remote-text">Text to type remotely</label><input id="remote-text" placeholder="Type text on the host, including non-Latin text" maxlength="4096"><button id="send-text" disabled>Send text</button></div>
      <p class="field-help">Click the image to focus keyboard input. Browser shortcuts may remain local. Control releases when the view loses focus.</p>
    </section>
    <section id="view-terminal" class="view" hidden>
      <div class="page-heading"><div><p class="eyebrow">HOST SHELL</p><h1>Terminal</h1><p class="lead">A real terminal, running as the host user.</p></div><span class="section-number">03 / TERMINAL</span></div>
      <div class="toolbar"><span class="mono small" id="terminal-status" role="status">No shell running</span><div class="toolbar-spacer"></div><button id="terminal-start" class="primary" data-connected disabled>Open terminal</button><button id="terminal-stop" disabled>Close terminal</button></div>
      <div class="terminal-frame"><div class="terminal-chrome"><span class="terminal-dot"></span><span>HOST / PTY</span><span id="terminal-size">80 × 24</span></div><div id="terminal"></div><div class="terminal-empty" id="terminal-empty">${icon("terminal")}<p>Open a terminal to begin.</p><span>The host must allow shell access.</span></div></div>
      <p class="field-help">Terminal size follows this window. Closing the terminal ends its host process.</p>
    </section>
    <section id="view-files" class="view" hidden>
      <div class="page-heading"><div><p class="eyebrow">SHARED DIRECTORY</p><h1>Files</h1><p class="lead">Work inside the directory shared by your host.</p></div><span class="section-number">04 / FILES</span></div>
      <form id="path-form" class="toolbar"><button type="button" id="files-up" aria-label="Parent directory" data-connected disabled>↑</button><label class="sr-only" for="file-path">Relative directory path</label><input id="file-path" value="." spellcheck="false" autocomplete="off"><button id="files-refresh" data-connected disabled>Browse</button><label class="button primary" for="upload" id="upload-label" aria-disabled="true">Upload file ↗</label><input type="file" id="upload" class="sr-only" disabled></form>
      <div class="file-table-wrap"><table><thead><tr><th scope="col">Name</th><th scope="col">Type</th><th scope="col" class="numeric">Size</th><th scope="col"><span class="sr-only">Actions</span></th></tr></thead><tbody id="file-entries"><tr><td colspan="4" class="empty-table">Connect to browse shared files.</td></tr></tbody></table></div>
      <div class="file-footer"><span id="file-count">0 items</span><span id="transfer-status" role="status">No transfer in progress</span></div>
      <form id="mkdir-form" class="text-input-row"><label class="sr-only" for="folder-name">New folder name</label><input id="folder-name" placeholder="New folder name" maxlength="255"><button data-connected disabled>Create folder</button></form>
      <p class="field-help">Uploads require host write permission and never overwrite an existing file.</p>
    </section>
  </main><footer class="app-footer"><span>LIGHTREMOTE</span><span id="footer-status">Your workspace, wherever you are.</span><span>V0.1</span></footer>
</div>`;

let link: Link | undefined,
  control: Control | undefined,
  connecting = false,
  generation = 0;
let connectionAbort: AbortController | undefined;
let desktop: MuxStream | undefined,
  inputStream: MuxStream | undefined,
  shell: MuxStream | undefined;
let desktopGeneration = 0,
  inputGeneration = 0,
  shellGeneration = 0;
let terminal: Terminal | undefined, fit: FitAddon | undefined;
let path = ".",
  fileBusy = false;
const canvas = $<HTMLCanvasElement>("desktop-canvas");
let view = "session";
function showError(error: unknown) {
  text("error-text", error instanceof Error ? error.message : String(error));
  $("error").hidden = false;
}
const attempt = (task: () => Promise<unknown>) => {
  void task().catch(showError);
};
button("dismiss-error").onclick = () => {
  $("error").hidden = true;
};
function status(state: string, label: string) {
  $("status").dataset.state = state;
  text("status-label", label);
  text(
    "footer-status",
    state === "connected"
      ? "Encrypted session established."
      : state === "waiting"
        ? "Establishing an encrypted session…"
        : "Your workspace, wherever you are.",
  );
}
function setConnected(connected: boolean) {
  document
    .querySelectorAll<HTMLButtonElement>("[data-connected]")
    .forEach((el) => {
      el.disabled = !connected;
    });
  input("upload").disabled = !connected;
  $("upload-label").setAttribute("aria-disabled", String(!connected));
  button("disconnect").disabled = !connected && !connecting;
  button("connect").disabled = connected || connecting;
  input("invitation").disabled = connected || connecting;
}
function requireLink(): Link {
  if (!link) throw new Error("Connect to a host first");
  return link;
}
function requireControl(): Control {
  if (!control) throw new Error("Connect to a host first");
  return control;
}
function navigate(name: string) {
  if (view === "desktop" && name !== "desktop") releaseInput();
  view = name;
  document.querySelectorAll<HTMLElement>(".view").forEach((el) => {
    el.hidden = el.id !== `view-${name}`;
  });
  document.querySelectorAll<HTMLButtonElement>("[data-view]").forEach((el) => {
    const active = el.dataset.view === name;
    el.classList.toggle("active", active);
    if (active) el.setAttribute("aria-current", "page");
    else el.removeAttribute("aria-current");
  });
  text("breadcrumb", name[0].toUpperCase() + name.slice(1));
  if (name === "terminal" && terminal) requestAnimationFrame(() => fit?.fit());
  if (name === "files" && control) attempt(() => listFiles(path));
}
document.querySelectorAll<HTMLButtonElement>("[data-view]").forEach((el) => {
  el.onclick = () => navigate(el.dataset.view!);
});
document.querySelector(".brand")!.addEventListener("click", (event) => {
  event.preventDefault();
  navigate("session");
});
function cleanup() {
  generation++;
  connectionAbort?.abort();
  connectionAbort = undefined;
  stopDesktop();
  stopShell();
  control?.close();
  control = undefined;
  const old = link;
  link = undefined;
  old?.close();
  connecting = false;
  setConnected(false);
  status("disconnected", "Disconnected");
  text("capability-status", "Session ended");
  text("room-name", "Not connected");
  $("capabilities").replaceChildren();
}
$("connect-form").onsubmit = (event) => {
  event.preventDefault();
  if (connecting || link) return;
  attempt(async () => {
    const invite = parseInvite(input("invitation").value);
    const current = ++generation;
    connectionAbort = new AbortController();
    connecting = true;
    setConnected(false);
    status("waiting", "Connecting");
    text("relay-name", new URL(invite.relay).host);
    text("room-name", invite.room_id);
    $("error").hidden = true;
    try {
      const connection = await Link.connect(
        invite,
        (error) => {
          if (current !== generation) return;
          cleanup();
          showError(error);
        },
        connectionAbort.signal,
      );
      if (current !== generation) {
        connection.close();
        return;
      }
      link = connection;
      const stream = await connection.open("control");
      if (current !== generation) {
        stream.reset();
        return;
      }
      control = new Control(
        stream,
        (event, data) => {
          if (event === "shell.exit") {
            text("terminal-status", `Shell exited: ${JSON.stringify(data)}`);
          }
        },
        (error) => {
          if (current === generation) {
            cleanup();
            showError(error);
          }
        },
      );
      connecting = false;
      setConnected(true);
      status("connected", "Connected");
      input("invitation").value = "";
      const capabilities = await control.call("capabilities");
      if (current !== generation) return;
      renderCapabilities(capabilities);
      text("capability-status", "Reported by host");
      text(
        "desktop-message",
        "Start a view. The host must support capture and allow desktop access.",
      );
    } catch (error) {
      if (current === generation) {
        cleanup();
        throw error;
      }
    }
  });
};
button("disconnect").onclick = cleanup;
function renderCapabilities(value: unknown) {
  const data = object(value);
  const region = $("capabilities");
  region.replaceChildren();
  for (const [key, value] of Object.entries(data)) {
    const item = document.createElement("div");
    item.className = "capability";
    const dot = document.createElement("span");
    dot.className = "capability-dot";
    dot.dataset.enabled = String(
      value === true || (typeof value === "object" && value !== null),
    );
    const title = document.createElement("span");
    title.textContent = key.replace(/_/g, " ");
    const detail = document.createElement("span");
    detail.className = "muted small";
    detail.textContent =
      typeof value === "boolean"
        ? value
          ? "Available"
          : "Unavailable"
        : typeof value === "object"
          ? JSON.stringify(value)
          : String(value);
    item.append(dot, title, detail);
    region.append(item);
  }
}

button("clipboard-get").onclick = () =>
  attempt(async () => {
    const result = await requireControl().call("clipboard.get");
    const value = typeof result === "string" ? result : object(result).text;
    if (typeof value !== "string")
      throw new Error("Host returned invalid clipboard text");
    input("clipboard").value = value;
    text("clipboard-status", "Read from host.");
  });
button("clipboard-set").onclick = () =>
  attempt(async () => {
    await requireControl().call("clipboard.set", {
      text: input("clipboard").value,
    });
    text("clipboard-status", "Sent to host.");
  });
button("clipboard-local").onclick = () =>
  attempt(async () => {
    if (!navigator.clipboard)
      throw new Error(
        "Local clipboard requires a secure context and browser permission",
      );
    input("clipboard").value = await navigator.clipboard.readText();
    text("clipboard-status", "Pasted from this device.");
  });
button("clipboard-copy").onclick = () =>
  attempt(async () => {
    if (!navigator.clipboard)
      throw new Error(
        "Local clipboard requires a secure context and browser permission",
      );
    await navigator.clipboard.writeText(input("clipboard").value);
    text("clipboard-status", "Copied to this device.");
  });

button("refresh-displays").onclick = () =>
  attempt(async () => {
    const result = await requireControl().call("desktop.displays");
    const displays = Array.isArray(result) ? result : object(result).displays;
    if (!Array.isArray(displays) || !displays.length)
      throw new Error("Host has no available displays");
    const select = $<HTMLSelectElement>("display");
    select.replaceChildren();
    for (const raw of displays) {
      const display = object(raw);
      const id = display.id ?? display.index;
      if (!Number.isInteger(id) || Number(id) < 0)
        throw new Error("Invalid host display ID");
      const option = document.createElement("option");
      option.value = String(id);
      option.textContent = `${String(display.name || `Display ${id}`)}${display.width && display.height ? ` · ${display.width} × ${display.height}` : ""}`;
      select.append(option);
    }
  });
$<HTMLSelectElement>("display").onchange = stopDesktop;
function stopDesktop() {
  desktopGeneration++;
  releaseInput();
  desktop?.reset("Desktop stopped");
  desktop = undefined;
  canvas.hidden = true;
  $("desktop-empty").hidden = false;
  text("desktop-status", "No desktop stream");
  button("desktop-stop").disabled = true;
  button("desktop-start").disabled = !link;
  input("enable-input").disabled = true;
}
async function renderJPEG(bytes: Uint8Array, current: number): Promise<void> {
  const expected = jpegDimensions(bytes);
  const image = await createImageBitmap(
    new Blob([Uint8Array.from(bytes)], { type: "image/jpeg" }),
  );
  try {
    if (current !== desktopGeneration) return;
    if (image.width !== expected.width || image.height !== expected.height)
      throw new Error("Decoded JPEG dimensions do not match its header");
    canvas.width = image.width;
    canvas.height = image.height;
    canvas.getContext("2d")!.drawImage(image, 0, 0);
    canvas.hidden = false;
    $("desktop-empty").hidden = true;
    text("desktop-status", `${image.width} × ${image.height} · Live`);
  } finally {
    image.close();
  }
}
button("desktop-start").onclick = () =>
  attempt(async () => {
    stopDesktop();
    const current = desktopGeneration;
    button("desktop-start").disabled = true;
    text("desktop-status", "Requesting desktop…");
    try {
      const stream = await requireLink().open("desktop", {
        display: Number(input("display").value),
        fps: 10,
        quality: 70,
        single: false,
      });
      if (current !== desktopGeneration) {
        stream.reset();
        return;
      }
      desktop = stream;
      button("desktop-stop").disabled = false;
      const records = new Records();
      let frames = 0;
      await stream.end();
      await stream.process((bytes) =>
        records.feed(bytes, async (bytes) => {
          if (current !== desktopGeneration) return;
          await renderJPEG(bytes, current);
          frames++;
          if (current === desktopGeneration)
            input("enable-input").disabled = false;
        }),
      );
      records.finish();
      if (current === desktopGeneration) {
        stopDesktop();
        text(
          "desktop-message",
          frames
            ? "The host ended the desktop stream."
            : "The host sent no desktop frames.",
        );
        throw new Error(
          frames
            ? "Desktop stream ended"
            : "Desktop unavailable: host sent no frames",
        );
      }
    } catch (error) {
      if (current === desktopGeneration) {
        stopDesktop();
        text(
          "desktop-message",
          error instanceof Error ? error.message : String(error),
        );
        throw error;
      }
    }
  });
button("desktop-stop").onclick = stopDesktop;
function saveBlob(blob: Blob, name: string) {
  const url = URL.createObjectURL(blob),
    a = document.createElement("a");
  a.href = url;
  a.download = name;
  a.click();
  setTimeout(() => URL.revokeObjectURL(url), 30_000);
}
button("screenshot").onclick = () =>
  attempt(async () => {
    button("screenshot").disabled = true;
    let stream: MuxStream | undefined;
    try {
      stream = await requireLink().open("desktop", {
        display: Number(input("display").value),
        fps: 1,
        quality: 90,
        single: true,
      });
      const records = new Records();
      let jpeg: Uint8Array<ArrayBuffer> | undefined;
      await stream.end();
      await stream.process((bytes) =>
        records.feed(bytes, (bytes) => {
          if (jpeg) throw new Error("Host sent multiple screenshot frames");
          jpegDimensions(bytes);
          jpeg = Uint8Array.from(bytes);
        }),
      );
      records.finish();
      if (!jpeg)
        throw new Error("Desktop unavailable: host sent no screenshot");
      saveBlob(
        new Blob([jpeg], { type: "image/jpeg" }),
        `lightremote-${Date.now()}.jpg`,
      );
    } catch (error) {
      stream?.reset("Screenshot failed");
      throw error;
    } finally {
      button("screenshot").disabled = !link;
    }
  });

const heldKeys = new Map<string, string>(),
  heldButtons = new Set<string>();
let inputWrites = Promise.resolve(),
  pendingMove: { x: number; y: number } | undefined,
  moveRAF = 0;
function releaseInput() {
  inputGeneration++;
  input("enable-input").checked = false;
  button("send-text").disabled = true;
  inputStream?.reset("Input released");
  inputStream = undefined;
  heldKeys.clear();
  heldButtons.clear();
  pendingMove = undefined;
  cancelAnimationFrame(moveRAF);
  moveRAF = 0;
  inputWrites = Promise.resolve();
}
input("enable-input").onchange = () => {
  if (!input("enable-input").checked) {
    releaseInput();
    return;
  }
  const current = ++inputGeneration;
  attempt(async () => {
    try {
      const stream = await requireLink().open("input", {
        display: Number(input("display").value),
      });
      if (current !== inputGeneration || !desktop) {
        stream.reset();
        return;
      }
      inputStream = stream;
      button("send-text").disabled = false;
      canvas.focus();
      void stream
        .process(() => {})
        .then(() => {
          if (inputStream === stream) {
            releaseInput();
            showError(new Error("Host closed remote input"));
          }
        })
        .catch((error) => {
          if (inputStream === stream) {
            releaseInput();
            showError(error);
          }
        });
    } catch (error) {
      if (current === inputGeneration) {
        releaseInput();
        throw error;
      }
    }
  });
};
let pendingInputs = 0;
function sendInput(value: unknown) {
  const stream = inputStream;
  if (!stream) return;
  if (pendingInputs >= 128) {
    releaseInput();
    showError(new Error("Input stalled; remote control released"));
    return;
  }
  pendingInputs++;
  inputWrites = inputWrites
    .then(async () => {
      if (inputStream === stream) await stream.write(jsonFrame(value));
    })
    .catch((error) => {
      if (inputStream === stream) {
        releaseInput();
        showError(error);
      }
    })
    .finally(() => {
      pendingInputs--;
    });
}
canvas.onpointermove = (event) => {
  if (!inputStream) return;
  pendingMove = pointerCoordinates(event);
  if (!moveRAF)
    moveRAF = requestAnimationFrame(() => {
      moveRAF = 0;
      if (pendingMove) sendInput({ type: "move", ...pendingMove });
      pendingMove = undefined;
    });
};
function pointerCoordinates(event: PointerEvent) {
  const rect = canvas.getBoundingClientRect();
  return {
    x: Math.max(
      0,
      Math.min(
        canvas.width - 1,
        Math.round(((event.clientX - rect.left) * canvas.width) / rect.width),
      ),
    ),
    y: Math.max(
      0,
      Math.min(
        canvas.height - 1,
        Math.round(((event.clientY - rect.top) * canvas.height) / rect.height),
      ),
    ),
  };
}
function flushMove() {
  if (pendingMove) {
    sendInput({ type: "move", ...pendingMove });
    pendingMove = undefined;
    cancelAnimationFrame(moveRAF);
    moveRAF = 0;
  }
}
canvas.onpointerdown = (event) => {
  if (!inputStream) return;
  event.preventDefault();
  canvas.focus();
  pendingMove = pointerCoordinates(event);
  flushMove();
  const name = ["left", "middle", "right"][event.button];
  if (!name) return;
  canvas.setPointerCapture(event.pointerId);
  heldButtons.add(name);
  sendInput({ type: "button", button: name, down: true });
};
canvas.onpointerup = (event) => {
  const name = ["left", "middle", "right"][event.button];
  if (name && heldButtons.delete(name))
    sendInput({ type: "button", button: name, down: false });
};
canvas.onpointercancel = releaseInput;
canvas.oncontextmenu = (event) => {
  if (inputStream) event.preventDefault();
};
canvas.addEventListener(
  "wheel",
  (event) => {
    if (!inputStream) return;
    event.preventDefault();
    const multiplier =
      event.deltaMode === 1 ? 16 : event.deltaMode === 2 ? canvas.height : 1;
    sendInput({
      type: "scroll",
      dx: Math.max(-100, Math.min(100, Math.round(event.deltaX * multiplier))),
      dy: Math.max(-100, Math.min(100, Math.round(event.deltaY * multiplier))),
    });
  },
  { passive: false },
);
canvas.onkeydown = (event) => {
  if (!inputStream || event.isComposing || event.key === "Unidentified") return;
  event.preventDefault();
  if (event.repeat || heldKeys.has(event.code)) return;
  const alreadyHeld = [...heldKeys.values()].includes(event.key);
  heldKeys.set(event.code, event.key);
  if (!alreadyHeld) sendInput({ type: "key", key: event.key, down: true });
};
canvas.onkeyup = (event) => {
  if (!inputStream) return;
  event.preventDefault();
  const key = heldKeys.get(event.code);
  heldKeys.delete(event.code);
  if (key && ![...heldKeys.values()].includes(key))
    sendInput({ type: "key", key, down: false });
};
canvas.onblur = () => {
  for (const key of new Set(heldKeys.values()))
    sendInput({ type: "key", key, down: false });
  heldKeys.clear();
  for (const name of heldButtons)
    sendInput({ type: "button", button: name, down: false });
  heldButtons.clear();
};
window.addEventListener("blur", releaseInput);
document.addEventListener("visibilitychange", () => {
  if (document.hidden) releaseInput();
});
button("send-text").onclick = () => {
  sendInput({ type: "text", text: input("remote-text").value });
  input("remote-text").value = "";
};

function ensureTerminal() {
  if (terminal) return;
  terminal = new Terminal({
    cursorBlink: true,
    screenReaderMode: true,
    fontSize: 14,
    fontFamily: '"Cascadia Code", "SFMono-Regular", Consolas, monospace',
    scrollback: 5000,
    theme: {
      background: "#101716",
      foreground: "#e3ece6",
      cursor: "#88dcc8",
      selectionBackground: "#385b52",
      black: "#192321",
      green: "#88dcc8",
    },
  });
  fit = new FitAddon();
  terminal.loadAddon(fit);
  terminal.open($("terminal"));
  fit.fit();
  terminal.onData((data) => {
    if (shell) {
      const stream = shell;
      void stream.write(new TextEncoder().encode(data)).catch((error) => {
        if (shell === stream) {
          stopShell();
          showError(error);
        }
      });
    }
  });
  let resizeTimer: ReturnType<typeof setTimeout>;
  terminal.onResize(({ cols, rows }) => {
    text("terminal-size", `${cols} × ${rows}`);
    clearTimeout(resizeTimer);
    const stream = shell;
    resizeTimer = setTimeout(() => {
      if (stream && shell === stream)
        attempt(async () => {
          await requireControl().call("shell.resize", {
            stream_id: stream.id,
            cols,
            rows,
          });
        });
    }, 100);
  });
  new ResizeObserver(() => {
    if (view === "terminal") fit?.fit();
  }).observe($("terminal"));
}
function stopShell() {
  shellGeneration++;
  shell?.reset("Terminal closed");
  shell = undefined;
  button("terminal-start").disabled = !link;
  button("terminal-stop").disabled = true;
  text("terminal-status", "No shell running");
}
button("terminal-start").onclick = () =>
  attempt(async () => {
    ensureTerminal();
    fit!.fit();
    terminal!.reset();
    const current = ++shellGeneration;
    button("terminal-start").disabled = true;
    text("terminal-status", "Starting shell…");
    try {
      const stream = await requireLink().open("shell", {
        cols: terminal!.cols,
        rows: terminal!.rows,
      });
      if (current !== shellGeneration) {
        stream.reset();
        return;
      }
      shell = stream;
      $("terminal-empty").hidden = true;
      button("terminal-stop").disabled = false;
      text("terminal-status", "Shell running");
      terminal!.focus();
      await stream.process(
        (bytes) =>
          new Promise<void>((resolve) => terminal!.write(bytes, resolve)),
      );
      if (current === shellGeneration) {
        await stream.end();
        shell = undefined;
        button("terminal-start").disabled = !link;
        button("terminal-stop").disabled = true;
        text("terminal-status", "Shell ended");
      }
    } catch (error) {
      if (current === shellGeneration) {
        stopShell();
        throw error;
      }
    }
  });
button("terminal-stop").onclick = stopShell;

function validName(name: string) {
  if (!name || name === "." || name === ".." || /[/\\\0:]/.test(name))
    throw new Error("Use a single file or folder name");
  return name;
}
function childPath(name: string) {
  validName(name);
  return path === "." ? name : `${path}/${name}`;
}
function bytesLabel(size: number): string {
  if (size < 1024) return `${size} B`;
  if (size < 1024 * 1024) return `${(size / 1024).toFixed(1)} KiB`;
  return `${(size / (1024 * 1024)).toFixed(1)} MiB`;
}
async function listFiles(directory: string) {
  const result = await requireControl().call("fs.list", { path: directory });
  const entries = Array.isArray(result) ? result : object(result).entries;
  if (!Array.isArray(entries) || entries.length > 4096)
    throw new Error("Invalid directory listing");
  path = directory || ".";
  input("file-path").value = path;
  const body = $("file-entries");
  body.replaceChildren();
  for (const raw of entries) {
    const entry = object(raw),
      name = String(entry.name),
      type = String(entry.type ?? entry.kind);
    validName(name);
    const row = document.createElement("tr");
    const title = document.createElement("td");
    if (type === "directory" || type === "dir") {
      const open = document.createElement("button");
      open.className = "file-name directory";
      open.textContent = `▱  ${name}`;
      open.onclick = () => attempt(() => listFiles(childPath(name)));
      title.append(open);
    } else title.textContent = name;
    const kind = document.createElement("td");
    kind.textContent = type;
    const size = document.createElement("td");
    size.className = "numeric mono";
    size.textContent =
      type === "file" ? bytesLabel(Number(entry.size || 0)) : "·";
    const actions = document.createElement("td");
    actions.className = "file-actions";
    if (type === "file") {
      const get = document.createElement("button");
      get.textContent = "Download";
      get.setAttribute("aria-label", `Download ${name}`);
      get.onclick = () => attempt(() => downloadFile(childPath(name), name));
      actions.append(get);
    }
    if (["file", "directory", "dir"].includes(type)) {
      const remove = document.createElement("button");
      remove.textContent = "Remove";
      remove.className = "quiet danger";
      remove.setAttribute("aria-label", `Remove ${name}`);
      remove.onclick = () => {
        if (confirm(`Remove ${name}? Directories must be empty.`))
          attempt(async () => {
            await requireControl().call("fs.remove", { path: childPath(name) });
            await listFiles(path);
          });
      };
      actions.append(remove);
    }
    row.append(title, kind, size, actions);
    body.append(row);
  }
  if (!entries.length) {
    const row = document.createElement("tr"),
      cell = document.createElement("td");
    cell.colSpan = 4;
    cell.className = "empty-table";
    cell.textContent = "This shared folder is empty.";
    row.append(cell);
    body.append(row);
  }
  text(
    "file-count",
    `${entries.length} ${entries.length === 1 ? "item" : "items"}`,
  );
}
$("path-form").onsubmit = (event) => {
  event.preventDefault();
  attempt(() => listFiles(input("file-path").value));
};
button("files-up").onclick = () =>
  attempt(() =>
    listFiles(
      path === "." ? "." : path.split("/").slice(0, -1).join("/") || ".",
    ),
  );
$("mkdir-form").onsubmit = (event) => {
  event.preventDefault();
  attempt(async () => {
    await requireControl().call("fs.mkdir", {
      path: childPath(input("folder-name").value),
    });
    input("folder-name").value = "";
    await listFiles(path);
  });
};
input("upload").onchange = () => {
  const file = input("upload").files?.[0];
  if (!file) return;
  attempt(async () => {
    if (fileBusy) throw new Error("Wait for the current transfer to finish");
    fileBusy = true;
    input("upload").disabled = true;
    try {
      const receipt = object(
        await upload(requireLink(), childPath(file.name), file, (n) =>
          text(
            "transfer-status",
            `Uploading ${bytesLabel(n)} / ${bytesLabel(file.size)}`,
          ),
        ),
      );
      text(
        "transfer-status",
        `Uploaded ${bytesLabel(file.size)} · SHA-256 ${String(receipt.sha256).slice(0, 12)}`,
      );
      await listFiles(path);
    } catch (error) {
      text("transfer-status", "Upload failed");
      throw error;
    } finally {
      fileBusy = false;
      input("upload").disabled = !link;
      input("upload").value = "";
    }
  });
};
interface SaveTarget {
  createWritable(): Promise<{
    write(bytes: Uint8Array): Promise<void>;
    close(): Promise<void>;
    abort(): Promise<void>;
  }>;
}
async function downloadFile(remotePath: string, name: string) {
  if (fileBusy) throw new Error("Wait for the current transfer to finish");
  fileBusy = true;
  const picker = (
    window as unknown as {
      showSaveFilePicker?: (options: {
        suggestedName: string;
      }) => Promise<SaveTarget>;
    }
  ).showSaveFilePicker;
  try {
    // Request the save destination while the user activation is still live.
    const sink = picker
      ? await (await picker({ suggestedName: name })).createWritable()
      : undefined;
    const blob = await download(
      requireLink(),
      remotePath,
      (n) => text("transfer-status", `Downloading ${bytesLabel(n)}`),
      sink,
    );
    if (blob) saveBlob(blob, name);
    text("transfer-status", `Downloaded ${name}`);
  } catch (error) {
    text("transfer-status", "Download cancelled or failed");
    throw error;
  } finally {
    fileBusy = false;
  }
}
window.addEventListener("pagehide", cleanup);
if (location.hash.length > 1) {
  input("invitation").value = location.href;
  history.replaceState(null, "", location.pathname + location.search);
  // Explicit Connect keeps permission and connection intent visible.
}
