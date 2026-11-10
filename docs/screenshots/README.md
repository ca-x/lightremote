# Interface screenshots

Screenshot source: 2026-11-10 local working-tree builds. Native images use the
actual v0.1.1 Linux GTK/X11 application running in isolated Xvfb `:101` at
1280×720, not design mockups or a physical desktop. The session uses a real
localhost relay and host. The host grants desktop permission, but no capture
request is made and input is not enabled. Screens contain no invitation secrets,
tickets or user data. Web images remain disconnected.

| File | Capture source | What it demonstrates |
| --- | --- | --- |
| `native-desktop.png` | Actual v0.1.1 Linux GTK/X11 app in Xvfb `:101`, 1280×720 | Disconnected native connection page, not Web-style cards |
| `native-session.png` | Same isolated Xvfb app, real localhost relay and host connection | Compact connected toolbar, workspace navigation and canvas filling remaining space; desktop permission granted, no capture request or input |
| `native-tools.png` | Same actual app and local session in isolated Xvfb | Tools popover actually expanded; no remote capture requested |
| `native-dark.png` | Actual app in isolated Xvfb with temporary GTK4 settings under an isolated `XDG_CONFIG_HOME` | Dark disconnected connection page; user configuration unchanged |
| `web-desktop.png` | Production Vite bundle in Chromium, 1440×1000 viewport (full page) | Web session workspace and shared static icons |
| `web-help.png` | Production Vite bundle in Chromium, 1440×1000 | Desktop pointer tooltip beside its named help control |
| `web-mobile.png` | Production Vite bundle in touch-enabled Chromium, 390×844 viewport (full page) | Responsive terminal layout with help expanded by tapping, no hover |

The native desktop controls use 32-point minimum heights; the separate
experimental Android viewer retains 48-point touch targets. Tools groups
infrequent desktop actions, including Save image with a system save dialog.

The mobile image is a full-page responsive **Web** screenshot, not an Android
emulator/device capture. No actual Android rendering, installation or remote
session is shown here. The connected native screenshot shows a real local
connection without Start viewing or Capture once. It is not evidence
of remote capture, received frames, input or an Android session. Linux images
do not establish macOS/Windows appearance, screen-reader support, signing or
real remote capture/input behavior. Web screenshots show the existing dark
theme. Native screenshots show light and dark connection pages; this does not
establish full light/dark parity across every workspace and interaction state.

## Refresh

Build and capture the browser views using the project's existing Playwright
dependency (requires Chromium):

```sh
npm --prefix web run screenshots
```

The command starts a local-only Vite preview on port 4174, captures disconnected
states, checks touch disclosure and tooltip visibility, and stops its server.
Set `CHROMIUM_PATH` if Chromium is not in a standard location.

For the Linux native images, build `roc-remote-native`, run an isolated Xvfb
display `:101` at 1280×720, and launch with `DISPLAY=:101 GDK_BACKEND=x11`.
Capture **only** the `roc-remote` app window with ImageMagick
`import -window roc-remote`, also using `DISPLAY=:101`. Record the capture date,
build revision and actual window dimensions. Capture the disconnected connection
page as `native-desktop.png`. For `native-session.png`, connect through a
localhost relay to a host with desktop permission granted and input disabled.
Clear the invitation from view, and do not invoke Start viewing or Capture once.
Record the actual connection and the absence of remote capture alongside the
image. Open Tools without invoking a capture action for `native-tools.png`.
For `native-dark.png`, use an isolated temporary `XDG_CONFIG_HOME` containing
`gtk-4.0/settings.ini` with `[Settings]` and
`gtk-interface-color-scheme=dark`; launch the app with that environment rather
than changing the user's GTK configuration. Verify the system save dialog
separately; an expanded Tools popover does not demonstrate saving a file.
Never capture the entire user desktop or a session containing real secrets.
