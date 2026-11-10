# Interface screenshots

Captured on 2026-11-10 from the local working tree after the Kubai Icons and
context-help changes. Screens show the actual application, not design mockups.
All sessions are disconnected, with no invitation, tickets or user data.

| File | Capture source | What it demonstrates |
| --- | --- | --- |
| `native-desktop.png` | Real Linux GTK/X11 app window, 1280×720 | Native workspace icons, labels and collapsed help in light appearance |
| `web-desktop.png` | Production Vite bundle in Chromium, 1440×1000 viewport (full page) | Web session workspace and shared static icons |
| `web-help.png` | Production Vite bundle in Chromium, 1440×1000 | Desktop pointer tooltip beside its named help control |
| `web-mobile.png` | Production Vite bundle in touch-enabled Chromium, 390×844 viewport (full page) | Responsive terminal layout with help expanded by tapping, no hover |

The mobile image is a full-page responsive **Web** screenshot, not an Android
emulator/device capture. The Android debug APK has been built, but no actual
Android rendering, installation or remote session is shown here. Linux images
do not establish macOS/Windows appearance, screen-reader support, signing or
real remote capture/input behavior. Web screenshots show the existing dark
theme; the native screenshot shows light appearance.

## Refresh

Build and capture the browser views using the project's existing Playwright
dependency (requires Chromium):

```sh
npm --prefix web run screenshots
```

The command starts a local-only Vite preview on port 4174, captures disconnected
states, checks touch disclosure and tooltip visibility, and stops its server.
Set `CHROMIUM_PATH` if Chromium is not in a standard location.

For the Linux native image, build `roc-remote-native`, launch it on an available
X11 display with `GDK_BACKEND=x11`, and capture **only** the `roc-remote` app
window with ImageMagick `import -window roc-remote`. This host uses `DISPLAY=:0`;
the window manager constrains the requested size to 1280×720. Never capture the
entire user desktop or a session containing real secrets for documentation.
