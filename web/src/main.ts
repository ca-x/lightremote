import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import "@xterm/xterm/css/xterm.css";
import "./style.css";
import { Link, type MuxStream, type LinkHealth } from "./link";
import { parseInvite } from "./secure";
import { Records, jsonFrame } from "./records";
import { jpegDimensions } from "./jpeg";
import { Control, object, upload, download } from "./client";
import {
  permissionDetails,
  scopes,
  parsePermissions,
  effectivePermission,
  acceptPermissions,
  type Scope,
  type PermissionSnapshot,
} from "./permissions";
import { watchUpdates } from "./updates";
declare const __APP_VERSION__: string;
const appVersion =
  typeof __APP_VERSION__ === "string" ? __APP_VERSION__ : undefined;

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
    <div id="network-notice" class="notice" role="status" hidden></div>
    <div id="update-notice" class="notice update-notice" role="status" hidden><span id="update-message"></span><button id="update-refresh">Refresh workspace</button></div>
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
      <section class="permissions-section" aria-labelledby="permissions-title"><div class="section-title"><h2 id="permissions-title">Session permissions</h2><span id="permission-mode" class="muted small">Host approval required</span></div><p class="muted">Choose what you need. Only the host can approve access.</p><form id="permissions-form"><div id="permission-list" class="permission-list"></div><div class="actions wrap"><button id="permissions-request" class="primary" disabled>Request selected permissions</button><button id="permissions-cancel" type="button" disabled>Cancel request</button></div><p id="permission-feedback" class="field-help" role="status">Connect to see available permissions.</p></form></section>
      <section class="clipboard-section"><div><p class="eyebrow">ON YOUR TERMS</p><h2>Clipboard</h2><p>Read or change the host clipboard only when you choose.</p></div><div><label for="clipboard">Clipboard text</label><textarea id="clipboard" rows="3" placeholder="Text to share with the host" maxlength="262144"></textarea><div class="actions wrap"><button id="clipboard-get" data-connected disabled>Read from host</button><button id="clipboard-set" data-connected disabled>Send to host</button><button id="clipboard-local">Paste from this device</button><button id="clipboard-copy">Copy to this device</button></div><p class="field-help" id="clipboard-status" role="status">No clipboard access requested.</p></div></section>
    </section>
    <section id="view-desktop" class="view" hidden>
      <div class="page-heading"><div><p class="eyebrow">REMOTE VIEW</p><h1>Desktop</h1><p class="lead">A live view, with control when you enable it.</p></div><span class="section-number">02 / DESKTOP</span></div>
      <div class="toolbar"><label class="inline-label">Display <select id="display" aria-label="Display"><option value="0">Display 0</option></select></label><button id="refresh-displays" data-connected disabled>Find displays</button><label class="inline-label">Bandwidth <select id="desktop-bandwidth" aria-label="Bandwidth" aria-describedby="bandwidth-help"><option value="standard">Standard</option><option value="low">Low bandwidth</option></select></label><div class="toolbar-spacer"></div><button id="desktop-start" class="primary" data-connected disabled>Start view</button><button id="desktop-stop" disabled>Stop</button><button id="screenshot" data-connected disabled>Save screenshot</button></div>
      <p class="field-help" id="bandwidth-help">Standard: 10 fps, quality 70. Low bandwidth: 3 fps, quality 40. Changes apply next time you start a view.</p>
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
      <form id="mkdir-form" class="text-input-row"><label class="sr-only" for="folder-name">New folder name</label><input id="folder-name" placeholder="New folder name" maxlength="255"><button id="mkdir" data-connected disabled>Create folder</button></form>
      <p class="field-help">Uploads require host write permission and never overwrite an existing file.</p>
    </section>
  </main><footer class="app-footer"><span>LIGHTREMOTE</span><span id="footer-status">Your workspace, wherever you are.</span><a href="https://github.com/lib-x/lightremote/releases/latest" target="_blank" rel="noopener noreferrer">Official releases ↗</a></footer>
</div>`;

let link: Link | undefined,
  control: Control | undefined,
  connecting = false,
  generation = 0;
let connectionAbort: AbortController | undefined;
let permissions: PermissionSnapshot | undefined,
  capabilities: Record<string, unknown> | undefined;
let permissionBusy = false;
let capabilityRefresh:
  | { owner: Control; dirty: boolean; promise: Promise<void> }
  | undefined;
let linkHealth: LinkHealth | undefined;
let transferAbort: AbortController | undefined,
  transferScope: Scope | undefined;
let screenshotAbort: AbortController | undefined;
const consumedInvites = new Set<string>();
function allowed(scope: Scope) {
  return !!link && effectivePermission(permissions, capabilities, scope);
}
function requirePermission(scope: Scope) {
  if (!navigator.onLine)
    throw new Error(
      "You are offline. Operations are not retried automatically.",
    );
  if (!allowed(scope))
    throw new Error(`Host approval required: ${permissionDetails[scope][0]}`);
}
function updateActions() {
  const groups: [Scope, string[]][] = [
    ["desktop", ["refresh-displays", "desktop-start", "screenshot"]],
    ["shell", ["terminal-start"]],
    ["file_read", ["files-up", "files-refresh"]],
    ["file_write", ["mkdir"]],
    ["clipboard_read", ["clipboard-get"]],
    ["clipboard_write", ["clipboard-set"]],
  ];
  for (const [scope, ids] of groups)
    for (const id of ids)
      button(id).disabled =
        !allowed(scope) ||
        !navigator.onLine ||
        (id === "desktop-start" && !!desktop) ||
        (id === "terminal-start" && !!shell) ||
        (id === "screenshot" && !!screenshotAbort);
  input("upload").disabled =
    !allowed("file_write") || fileBusy || !navigator.onLine;
  $("upload-label").setAttribute(
    "aria-disabled",
    String(input("upload").disabled),
  );
  input("enable-input").disabled =
    !allowed("input") || !desktop || canvas.hidden || !navigator.onLine;
  button("send-text").disabled =
    !allowed("input") || !inputStream || !navigator.onLine;
  for (const el of document.querySelectorAll<HTMLButtonElement>(
    "[data-file-scope]",
  ))
    el.disabled = !allowed(el.dataset.fileScope as Scope) || !navigator.onLine;
}
function renderPermissions() {
  const region = $("permission-list");
  const selected = new Set(
    Array.from(
      region.querySelectorAll<HTMLInputElement>("input:checked"),
      (el) => el.value,
    ),
  );
  region.replaceChildren();
  for (const scope of scopes) {
    const row = document.createElement("label");
    row.className = "permission-row";
    const check = document.createElement("input");
    check.type = "checkbox";
    check.value = scope;
    check.id = `permission-${scope}`;
    check.setAttribute("aria-label", permissionDetails[scope][0]);
    check.setAttribute("aria-describedby", `permission-description-${scope}`);
    check.checked = selected.has(scope);
    check.disabled =
      !permissions?.available.includes(scope) ||
      permissions.granted.includes(scope) ||
      !!permissions.pending ||
      permissionBusy ||
      !navigator.onLine;
    if (check.disabled) check.checked = false;
    const detail = document.createElement("span"),
      title = document.createElement("strong"),
      description = document.createElement("span");
    title.textContent = permissionDetails[scope][0];
    description.id = `permission-description-${scope}`;
    description.textContent = permissionDetails[scope][1];
    description.className = "muted small";
    detail.append(title, description);
    const state = document.createElement("span");
    state.className = "permission-state small";
    state.dataset.scope = scope;
    state.textContent = !permissions
      ? "Not connected"
      : permissions.granted.includes(scope)
        ? allowed(scope)
          ? "Granted"
          : "Granted · unavailable"
        : permissions.pending?.scopes.includes(scope)
          ? "Awaiting host"
          : permissions.available.includes(scope)
            ? "Not granted"
            : "Unavailable";
    check.onchange = updatePermissionButtons;
    row.append(check, detail, state);
    region.append(row);
  }
  text(
    "permission-mode",
    permissions?.mode === "unattended"
      ? "Unattended host policy"
      : "Host approval required",
  );
  updatePermissionButtons();
}
function updatePermissionButtons() {
  button("permissions-request").disabled =
    !control ||
    permissionBusy ||
    !!permissions?.pending ||
    !navigator.onLine ||
    !$("permission-list").querySelector("input:checked");
  button("permissions-cancel").disabled =
    !control || permissionBusy || !permissions?.pending || !navigator.onLine;
}
function enforcePermissions() {
  if (!allowed("desktop")) {
    stopDesktop();
    screenshotAbort?.abort();
  }
  if (!allowed("input")) releaseInput();
  if (!allowed("shell")) {
    stopShell();
    terminal?.clear();
    $("terminal-empty").hidden = false;
  }
  if (transferScope && !allowed(transferScope)) transferAbort?.abort();
  if (!allowed("file_read")) {
    $("file-entries").replaceChildren();
    text("file-count", "0 items");
  }
  updateActions();
}
function receivePermissions(value: unknown) {
  const next = parsePermissions(value);
  if (!acceptPermissions(permissions, next)) return;
  const previous = permissions;
  control?.setPermissions(next);
  permissions = next;
  if (
    previous?.granted.includes("clipboard_read") &&
    !next.granted.includes("clipboard_read")
  ) {
    input("clipboard").value = "";
    text(
      "clipboard-status",
      "Host clipboard read permission revoked. Received text was cleared.",
    );
  }
  if (
    previous?.granted.includes("desktop") &&
    !next.granted.includes("desktop")
  )
    text(
      "desktop-message",
      "Desktop access was revoked. Request permission in Session to start another view.",
    );
  const decision = next.last_decision;
  if (
    decision &&
    (decision.id !== previous?.last_decision?.id ||
      decision.outcome !== previous?.last_decision?.outcome)
  ) {
    text(
      "permission-feedback",
      decision.outcome === "approved"
        ? "The host approved your request."
        : decision.outcome === "denied"
          ? "The host denied your request. You can choose permissions and request again."
          : "Request cancelled. No additional access was granted.",
    );
  } else if (next.pending)
    text(
      "permission-feedback",
      "Waiting for the host to approve. You can cancel this request.",
    );
  else if (previous?.granted.some((scope) => !next.granted.includes(scope)))
    text(
      "permission-feedback",
      "The host revoked access. Affected tools and transfers have stopped.",
    );
  else if (!previous)
    text(
      "permission-feedback",
      next.granted.length
        ? "Access follows the host’s current grants."
        : "Select permissions to ask the host for access.",
    );
  enforcePermissions();
  renderPermissions();
}
function refreshCapabilities(): Promise<void> {
  const owner = requireControl();
  if (capabilityRefresh?.owner === owner) {
    capabilityRefresh.dirty = true;
    return capabilityRefresh.promise;
  }
  const current = generation;
  const refresh = { owner, dirty: false, promise: Promise.resolve() };
  capabilityRefresh = refresh;
  refresh.promise = (async () => {
    do {
      refresh.dirty = false;
      const data = object(await owner.call("capabilities"));
      if (current !== generation || owner !== control) return;
      const snapshot = parsePermissions(
        data.permissions ?? (await owner.call("permissions.get")),
      );
      if (current !== generation) return;
      if (!acceptPermissions(permissions, snapshot)) continue;
      capabilities = data;
      receivePermissions(snapshot);
      renderCapabilities(data);
    } while (refresh.dirty && current === generation);
  })().finally(() => {
    if (capabilityRefresh === refresh) capabilityRefresh = undefined;
  });
  return refresh.promise;
}
$("permissions-form").onsubmit = (event) => {
  event.preventDefault();
  if (permissionBusy || permissions?.pending) return;
  const requested = Array.from(
    $("permission-list").querySelectorAll<HTMLInputElement>("input:checked"),
    (el) => el.value,
  );
  if (!requested.length) return;
  const current = generation;
  permissionBusy = true;
  renderPermissions();
  attempt(async () => {
    try {
      const snapshot = await requireControl().call("permissions.request", {
        scopes: requested,
      });
      if (current !== generation) return;
      receivePermissions(snapshot);
      await refreshCapabilities();
    } finally {
      if (current === generation) {
        permissionBusy = false;
        renderPermissions();
      }
    }
  });
};
button("permissions-cancel").onclick = () => {
  const id = permissions?.pending?.id,
    current = generation;
  if (id === undefined || permissionBusy) return;
  permissionBusy = true;
  renderPermissions();
  attempt(async () => {
    try {
      const snapshot = await requireControl().call("permissions.cancel", {
        id,
      });
      if (current !== generation) return;
      receivePermissions(snapshot);
      if (
        permissions?.last_decision?.id === id &&
        permissions.last_decision.outcome === "cancelled"
      )
        text(
          "permission-feedback",
          "Request cancelled. No additional access was granted.",
        );
    } finally {
      if (current === generation) {
        permissionBusy = false;
        renderPermissions();
      }
    }
  });
};
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
  updateActions();
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
  if (name === "files" && allowed("file_read")) attempt(() => listFiles(path));
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
  transferAbort?.abort();
  screenshotAbort?.abort();
  permissions = undefined;
  capabilities = undefined;
  permissionBusy = false;
  stopDesktop();
  stopShell();
  control?.close();
  control = undefined;
  const old = link;
  link = undefined;
  old?.close();
  connecting = false;
  linkHealth = undefined;
  setConnected(false);
  renderNetwork();
  text("capability-status", "Session ended");
  text("room-name", "Not connected");
  $("capabilities").replaceChildren();
  input("clipboard").value = "";
  text("clipboard-status", "No clipboard access requested.");
  $("file-entries").replaceChildren();
  text("file-count", "0 items");
  terminal?.clear();
  $("terminal-empty").hidden = false;
  renderPermissions();
  text(
    "permission-feedback",
    "Session ended. Connect with a fresh invitation to request access.",
  );
}
$("connect-form").onsubmit = (event) => {
  event.preventDefault();
  if (connecting || link) return;
  attempt(async () => {
    if (!navigator.onLine)
      throw new Error(
        "You are offline. Connect when your network returns, using a fresh invitation.",
      );
    const invite = parseInvite(input("invitation").value);
    const invitationKey = `${invite.relay}|${invite.room_id}|${invite.client_token}`;
    if (consumedInvites.has(invitationKey))
      throw new Error(
        "This invitation has already been used. Ask the host for a fresh invitation.",
      );
    if (consumedInvites.size >= 64)
      throw new Error("Reload the workspace before starting another session.");
    consumedInvites.add(invitationKey);
    input("invitation").value = "";
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
      connection.onHealth = (health) => {
        if (current !== generation) return;
        linkHealth = health;
        renderNetwork();
      };
      linkHealth = connection.health;
      const stream = await connection.open("control");
      if (current !== generation) {
        stream.reset();
        return;
      }
      control = new Control(
        stream,
        (event, data) => {
          if (current !== generation) return;
          if (event === "permissions.changed") {
            try {
              receivePermissions(data);
            } catch (error) {
              cleanup();
              showError(error);
              return;
            }
            attempt(refreshCapabilities);
          }
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
      renderNetwork();
      input("invitation").value = "";
      await refreshCapabilities();
      if (current !== generation) return;
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
    if (key === "permissions" || typeof value === "object") continue;
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
    requirePermission("clipboard_read");
    const current = generation,
      revision = permissions?.revision;
    const result = await requireControl().call("clipboard.get");
    if (
      current !== generation ||
      revision !== permissions?.revision ||
      !allowed("clipboard_read")
    )
      return;
    const value = typeof result === "string" ? result : object(result).text;
    if (typeof value !== "string")
      throw new Error("Host returned invalid clipboard text");
    input("clipboard").value = value;
    text("clipboard-status", "Read from host.");
  });
button("clipboard-set").onclick = () =>
  attempt(async () => {
    requirePermission("clipboard_write");
    const current = generation,
      revision = permissions?.revision;
    await requireControl().call("clipboard.set", {
      text: input("clipboard").value,
    });
    if (
      current !== generation ||
      revision !== permissions?.revision ||
      !allowed("clipboard_write")
    )
      return;
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
    requirePermission("desktop");
    const current = generation,
      revision = permissions?.revision;
    const result = await requireControl().call("desktop.displays");
    if (
      current !== generation ||
      revision !== permissions?.revision ||
      !allowed("desktop")
    )
      return;
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
  button("desktop-start").disabled = !allowed("desktop");
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
    requirePermission("desktop");
    stopDesktop();
    const current = desktopGeneration;
    button("desktop-start").disabled = true;
    text("desktop-status", "Requesting desktop…");
    try {
      const lowBandwidth =
        $<HTMLSelectElement>("desktop-bandwidth").value === "low";
      const stream = await requireLink().open("desktop", {
        display: Number(input("display").value),
        fps: lowBandwidth ? 3 : 10,
        quality: lowBandwidth ? 40 : 70,
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
            input("enable-input").disabled = !allowed("input");
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
    requirePermission("desktop");
    if (screenshotAbort) return;
    button("screenshot").disabled = true;
    const abort = new AbortController();
    screenshotAbort = abort;
    const cancel = () => stream?.reset("Screenshot cancelled");
    abort.signal.addEventListener("abort", cancel, { once: true });
    let stream: MuxStream | undefined;
    try {
      stream = await requireLink().open("desktop", {
        display: Number(input("display").value),
        fps: 1,
        quality: 90,
        single: true,
      });
      if (abort.signal.aborted) cancel();
      abort.signal.throwIfAborted();
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
      abort.signal.throwIfAborted();
      requirePermission("desktop");
      saveBlob(
        new Blob([jpeg], { type: "image/jpeg" }),
        `lightremote-${Date.now()}.jpg`,
      );
    } catch (error) {
      stream?.reset("Screenshot failed");
      throw error;
    } finally {
      abort.signal.removeEventListener("abort", cancel);
      if (screenshotAbort === abort) screenshotAbort = undefined;
      button("screenshot").disabled = !allowed("desktop");
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
  if (!allowed("input") || !navigator.onLine) {
    releaseInput();
    return;
  }
  const current = ++inputGeneration;
  attempt(async () => {
    try {
      const stream = await requireLink().open("input", {
        display: Number(input("display").value),
      });
      if (current !== inputGeneration || !desktop || !allowed("input")) {
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
  const action = value as { type?: string; down?: boolean };
  const release =
    (action.type === "key" || action.type === "button") &&
    action.down === false;
  if (!release && (!allowed("input") || !navigator.onLine)) return;
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
      if (
        inputStream === stream &&
        (release || (allowed("input") && navigator.onLine))
      )
        await stream.write(jsonFrame(value));
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
    if (shell && allowed("shell") && navigator.onLine) {
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
  button("terminal-start").disabled = !allowed("shell");
  button("terminal-stop").disabled = true;
  text("terminal-status", "No shell running");
}
button("terminal-start").onclick = () =>
  attempt(async () => {
    requirePermission("shell");
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
        button("terminal-start").disabled = !allowed("shell");
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
  requirePermission("file_read");
  const current = generation,
    revision = permissions?.revision;
  const result = await requireControl().call("fs.list", { path: directory });
  if (
    current !== generation ||
    revision !== permissions?.revision ||
    !allowed("file_read")
  )
    return;
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
      get.dataset.fileScope = "file_read";
      get.setAttribute("aria-label", `Download ${name}`);
      get.onclick = () => attempt(() => downloadFile(childPath(name), name));
      actions.append(get);
    }
    if (["file", "directory", "dir"].includes(type)) {
      const remove = document.createElement("button");
      remove.textContent = "Remove";
      remove.dataset.fileScope = "file_write";
      remove.disabled = !allowed("file_write");
      remove.className = "quiet danger";
      remove.setAttribute("aria-label", `Remove ${name}`);
      remove.onclick = () => {
        if (confirm(`Remove ${name}? Directories must be empty.`))
          attempt(async () => {
            requirePermission("file_write");
            const current = generation,
              revision = permissions?.revision;
            await requireControl().call("fs.remove", { path: childPath(name) });
            if (
              current !== generation ||
              revision !== permissions?.revision ||
              !allowed("file_write")
            )
              return;
            if (allowed("file_read")) await listFiles(path);
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
    requirePermission("file_write");
    const current = generation,
      revision = permissions?.revision;
    await requireControl().call("fs.mkdir", {
      path: childPath(input("folder-name").value),
    });
    if (
      current !== generation ||
      revision !== permissions?.revision ||
      !allowed("file_write")
    )
      return;
    input("folder-name").value = "";
    if (allowed("file_read")) await listFiles(path);
  });
};
input("upload").onchange = () => {
  const file = input("upload").files?.[0];
  if (!file) return;
  attempt(async () => {
    requirePermission("file_write");
    if (fileBusy) throw new Error("Wait for the current transfer to finish");
    const abort = new AbortController();
    transferAbort = abort;
    transferScope = "file_write";
    fileBusy = true;
    input("upload").disabled = true;
    try {
      const receipt = object(
        await upload(
          requireLink(),
          childPath(file.name),
          file,
          (n) =>
            text(
              "transfer-status",
              `Uploading ${bytesLabel(n)} / ${bytesLabel(file.size)}`,
            ),
          abort.signal,
        ),
      );
      abort.signal.throwIfAborted();
      text(
        "transfer-status",
        `Uploaded ${bytesLabel(file.size)} · SHA-256 ${String(receipt.sha256).slice(0, 12)}`,
      );
      if (allowed("file_read")) await listFiles(path);
    } catch (error) {
      text(
        "transfer-status",
        abort.signal.aborted
          ? "Upload cancelled: session or permission changed"
          : "Upload failed. Check the host before trying again.",
      );
      throw error;
    } finally {
      fileBusy = false;
      if (transferAbort === abort) {
        transferAbort = undefined;
        transferScope = undefined;
      }
      updateActions();
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
  requirePermission("file_read");
  if (fileBusy) throw new Error("Wait for the current transfer to finish");
  fileBusy = true;
  const abort = new AbortController();
  transferAbort = abort;
  transferScope = "file_read";
  updateActions();
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
      abort.signal,
    );
    abort.signal.throwIfAborted();
    if (blob) saveBlob(blob, name);
    text("transfer-status", `Downloaded ${name}`);
  } catch (error) {
    text("transfer-status", "Download cancelled or failed");
    throw error;
  } finally {
    fileBusy = false;
    if (transferAbort === abort) {
      transferAbort = undefined;
      transferScope = undefined;
    }
    updateActions();
  }
}
function renderNetwork() {
  $("status").title =
    linkHealth?.rtt_ms != null
      ? `Round-trip ${Math.round(linkHealth.rtt_ms)} ms`
      : "";
  const offline = !navigator.onLine,
    weak = linkHealth?.phase === "degraded";
  $("network-notice").hidden = !offline && !weak;
  text(
    "network-notice",
    offline
      ? "You are offline. No operations will be retried. A closed session needs a fresh invitation."
      : "Weak network: responses may be delayed. Remote control has been released. Operations are not retried automatically.",
  );
  if (offline) status("offline", "Offline");
  else if (weak) {
    releaseInput();
    status("weak", "Weak network");
  } else
    status(
      link ? "connected" : connecting ? "waiting" : "disconnected",
      link ? "Connected" : connecting ? "Connecting" : "Disconnected",
    );
}
function networkChanged() {
  renderNetwork();
  if (!navigator.onLine) releaseInput();
  updateActions();
  renderPermissions();
}
window.addEventListener("online", networkChanged);
window.addEventListener("offline", networkChanged);
const watchWorkspaceUpdates = () =>
  watchUpdates({
    currentVersion: appVersion,
    online: () => navigator.onLine,
    onUpdate(version) {
      text(
        "update-message",
        `Workspace ${version} is deployed and ready. Refresh when you are ready.`,
      );
      $("update-notice").hidden = false;
    },
  });
let updates = watchWorkspaceUpdates();
window.addEventListener("pageshow", (event) => {
  if (event.persisted) {
    updates.stop();
    updates = watchWorkspaceUpdates();
  }
});
button("update-refresh").onclick = () => {
  if (!navigator.onLine) {
    showError(new Error("Reconnect to the network before refreshing."));
    return;
  }
  if (
    !confirm(
      link || connecting || fileBusy
        ? "Disconnect this session, cancel any transfers and refresh the workspace? You will need a fresh invitation."
        : "Refresh the workspace to load the deployed update?",
    )
  )
    return;
  cleanup();
  updates.stop();
  location.reload();
};
window.addEventListener("pagehide", () => {
  cleanup();
  updates.stop();
});
networkChanged();
if (location.hash.length > 1) {
  input("invitation").value = location.href;
  history.replaceState(null, "", location.pathname + location.search);
  // Explicit Connect keeps permission and connection intent visible.
}
