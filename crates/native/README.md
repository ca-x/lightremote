# LightRemote native controller

A native Rust NSH application. It calls `lightremote-core` directly; it does not
embed the browser client or start an installed NSH CLI. The Rust sources and OS
platform libraries supply the UI; there are no sidecar web assets to package.

## Build and run

From the repository root:

```sh
cargo build --locked --release -p lightremote-native
cargo run --locked -p lightremote-native
cargo test --locked -p lightremote-native
cargo clippy --locked -p lightremote-native --all-targets --no-deps -- -D warnings
```

All NSH crates use the same git commit:
`0e4a20bd41ee72a35515cecbf9a7fba88742d911` (0.4.0). The three platform
dependencies disable default features. `nsh-base` enables only the `term` feature
on top of its core controls. No QuickJS, WebView,
NSH CLI, npm build, or browser crypto is needed by this controller. Rust performs
transport encryption through the shared core. Headless CLI/service builds should
continue to exclude this crate; a runtime switch does not remove GUI libraries.

Build on each target OS; cross-OS native builds have not been verified.

- Linux: GTK 4.14+, pkg-config, clang, GStreamer and its base development
  libraries. Ubuntu 24.04: `libgtk-4-dev clang pkg-config libgstreamer1.0-dev
  libgstreamer-plugins-base1.0-dev`. GTK/Pango/GStreamer runtime libraries remain
  required on the installed machine. WebKit is not required.
- Windows: the MSVC toolchain, C++ build tools and Windows SDK. NSH targets
  Windows 10 1809+. This Rust-only configuration does not use WebView2.
- macOS: Xcode command-line tools, macOS 14+. Set
  `MACOSX_DEPLOYMENT_TARGET=14.0` when creating release artifacts.

Package the resulting executable, license information and the distribution's
signing status. macOS app bundles/signing/notarization and Windows signing are
separate release steps. The official `nsh package` path applies to NSH CLI app
projects; the standalone Rust binary here builds with Cargo.

## Operations and bounds

Paste a full invitation URL or fragment into the masked Invitation field and
connect. The field clears on submission; the connection can be cancelled.
Connection failures, operation rejection and remote stream reset appear in the
persistent header. Desktop, Terminal, Files, Clipboard and Session are separate
keyboard-navigable workspaces; switching tools retains drafts and the terminal,
but releases remote input. Session → Refresh permissions queries the host's
permissions. Remote actions stay disabled until their permission is granted.
Desktop → Find displays populates the labeled display picker without replacing
permission state. Files uses a virtualized name/type/size directory list.

The app owns its light/dark tokens and styled NSH controls; see
[native UI design](../../docs/native-ui.md). High-frequency navigation and input
have immediate feedback without ornamental motion or artificial delays.

Start viewing opens the `desktop` route at 10 FPS / JPEG quality 70. Capture once opens
that route with `single:true` and validates successful stream completion before
making the frame exportable. Save image writes the most recent complete JPEG
record to the explicit local path, via a same-directory temporary file, without
overwriting an existing file. A latest-value watch channel replaces old frames;
frames obey core's 8 MiB limit. Shared header-only JPEG admission also limits
each edge to 16384 and the actual JPEG raster to 16,777,216 pixels, independently
of peer display metadata. The check runs before publishing frames and again
before each platform decoder. All native routes read/write or split the whole
error-aware `MuxStream`, so RESET or bridge failure is returned as an I/O error
rather than ordinary EOF. File publication also waits for `completed()`.

Click the image to focus remote control. NSH registers a Rust native desktop
component for each OS: GTK Picture/event controllers, an AppKit NSImageView
subclass, or a Win32 child window. Its `on_native_event` Rust callback forwards
pointer motion, left/right/middle button transitions, fractional wheel deltas,
and keyboard down/up events to one ordered `input` stream. Text injection remains
available separately. Coordinates exclude letterboxing and scale from local
widget units to the selected display's pixels. `desktop.displays` supplies the
ID, dimensions and global offset; input OPEN metadata carries that ID. Wire
coordinates are display-local: the host input service adds x/y offsets exactly
once, including for monitors left of or above the primary display.

Focus loss, window deactivation, changing/stopping the desktop and disconnect
release remote input. Release has a separate epoch signal, so it cannot be lost
behind the input queue; stale queued input is discarded, not replayed on refocus.
Held buttons/keys are explicitly released before FIN, with a two-second cleanup
bound. If the transport fails, the host's stream teardown releases its held input.
The 128-slot input queue pauses control on overflow; clicking refocuses it.
OS-reserved shortcuts (for example Ctrl+Alt+Delete) remain subject to local OS
security rules. IME text can use the explicit text field.

Clipboard provides a bounded multiline text editor and explicit Read remote /
Write remote controls, using `clipboard.get` / `clipboard.set` on the existing
control stream. Host permission failures are shown unchanged. Text is limited to
256 KiB of UTF-8 with no NUL, and writes must also fit the control frame after
JSON escaping. Reading replaces the editor only on success; a failed read leaves
its previous contents intact. Disconnect clears remote clipboard data from the
editor. Empty text is allowed, so Write remote can explicitly clear it.
Validation appears beside the editor; invalid drafts cannot be written remotely.

Paste local explicitly reads the OS clipboard through NSH's `clipboard_string`
into the editor; Copy editor locally calls NSH's `set_clipboard`. Neither action
contacts the host. Read remote does not alter the local OS clipboard, and Write
remote sends only the editor text. No clipboard data is read on connection, no
periodic reads or synchronization run, and there are no image/file clipboard
transfers. NSH returns empty text when the local clipboard has none; its local
clipboard setters have no completion/error receipt, so the UI does not claim a
verified OS clipboard write.

Shell uses NSH's native `Terminal`/VT emulator, keyboard events, 1000 lines of
scrollback, and `shell.resize` synchronization with its actual cell dimensions.
PTY output is fed incrementally; UTF-8 sequences are retained across reads. The
UI handoff retains 128 KiB of recent output; if rendering falls further behind,
the terminal explicitly reports and resets the display rather than silently
pretending its VT state is intact. Closing the shell cancels its stream.
Disconnect cancels running operations after input release cleanup.

File paths on the remote side are relative to the host's configured share root.
List queries `fs.list`. Upload and download use the path fields rather than a
platform file picker. Transfers are bounded to 512 MiB and use streamed I/O.
Downloads use a same-directory temporary file and publish without overwriting
only after successful remote completion; cancellation removes the temporary
file. Upload hashes the actual streamed bytes, checks both receipt size and
SHA-256, preserves remote error code/message, and validates terminal stream
success. The host owns authorization, safe path resolution and upload
atomicity. The client does not implement directory creation/removal.

The command queue has 16 slots; at most 24 operation tasks are active, with a
separate 128-slot ordered input queue and a non-droppable release signal. UI and
Tokio run on separate threads. Connect/control requests have a 25 second timeout;
long-running media/shell/transfers remain cancellable through Stop/Close or
Disconnect. The core limits stream count and carrier queues independently.

## Verification boundaries

Linux is built and tested against the actual pinned NSH framework. A loopback
relay test exercises the real encrypted TMP input stream through blur, release
records, FIN and completion. Unit tests cover display scaling/offsets, bounds,
input queue epochs, screenshot export/no-overwrite, UTF-8, control framing and
receipt errors/checksums.

UI redesign verification on 2026-11-09: native Linux Cargo check, build and
Clippy pass. NSH headless tests cover disconnected and connecting controls,
per-permission actions, keyboard navigation, retained drafts, the display picker
and directory-cell recycling in light/dark and reduced-motion configurations.
CLI loopback end-to-end, Web checks and Cloudflare unit checks are separate
evidence, not proof of native on-screen rendering. Launching the actual GTK
window in this session fails with `Failed to open display`; no screenshot or
real-desktop acceptance is claimed. `scripts/check.sh` passes its headless
checks but stops in the default desktop-host build because `libclang` is missing.
This does not invalidate the separate NSH build, but the full script is not green.

Full Windows/macOS `cargo check` on this Linux host stops in ring's native C
build (missing MinGW / Apple SDK). Separate compile-only probes checked these
surface modules against the matching Win32/AppKit bindings and the published
NSH native-component trait signatures, using API stand-ins for NSH's platform
host. Those checks do not constitute full NSH target builds, signing, real
window event testing or physical remote desktop validation. Actual per-OS CI
and interactive acceptance remain required.

## Primary sources

- [Official Rust example entry point](https://github.com/longbridge/nsh/blob/0e4a20bd41ee72a35515cecbf9a7fba88742d911/examples/tasks-rs/src/main.rs)
- [Official installation and per-platform prerequisites](https://github.com/longbridge/nsh/blob/0e4a20bd41ee72a35515cecbf9a7fba88742d911/website/docs/installation.md)
- [Linux platform features/dependencies](https://github.com/longbridge/nsh/blob/0e4a20bd41ee72a35515cecbf9a7fba88742d911/crates/nsh-platform-linux/Cargo.toml)
- [Official native Rust CI matrix](https://github.com/longbridge/nsh/blob/0e4a20bd41ee72a35515cecbf9a7fba88742d911/.github/workflows/rust-app.yml)
- [NSH Terminal implementation](https://github.com/longbridge/nsh/blob/0e4a20bd41ee72a35515cecbf9a7fba88742d911/crates/nsh-base/src/term/mod.rs)
- [Native component API](https://github.com/longbridge/nsh/blob/0e4a20bd41ee72a35515cecbf9a7fba88742d911/crates/nsh-platform-linux/src/native.rs)
- [WebView HTML/message bridge](https://github.com/longbridge/nsh/blob/0e4a20bd41ee72a35515cecbf9a7fba88742d911/website/modules/nsh-webview.md)

Capture/input are host capabilities and are deliberately outside this controller.
The API survey identified `xcap = "=0.9.8"` and `enigo = "=0.6.1"` for a cross-OS
MVP: `xcap::Monitor::all()?.remove(0).capture_image()?` and
`Enigo::new(&Settings::default())?.move_mouse(x, y, Coordinate::Abs)?` (import
`enigo::Mouse`, and check for an empty monitor list first). These signatures were
checked against the published crate sources; the separate Linux combined probe
was blocked by missing `libclang.so` for PipeWire bindgen. Do not treat that probe
as a successful capture build or physical desktop test.

- [xcap 0.9.8 Monitor API](https://docs.rs/xcap/0.9.8/xcap/struct.Monitor.html)
- [enigo 0.6.1 API](https://docs.rs/enigo/0.6.1/enigo/)
- [ScreenCast portal](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.ScreenCast.html)
- [RemoteDesktop portal](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.RemoteDesktop.html)

Wayland continuous capture should use an authorized ScreenCast/PipeWire session;
GNOME/KDE input needs RemoteDesktop portal/libei or an explicitly authorized
platform backend. wlroots input protocols are not portable to every compositor.
macOS requires screen-recording/accessibility permissions; Windows elevation and
secure desktop boundaries need separate testing. Screenshot support alone is not
a low-latency video capture guarantee.
