# Platform setup and limits

The portable host and native controller are separate executables. Desktop support
is an optional host feature; `cargo build -p lightremote --no-default-features`
requires no capture, injection or GUI libraries.

## Linux

Host desktop capture uses xcap; input uses enigo's X11 backend. X11 is the first
backend to validate. Wayland compositors may expose a capture portal, but native
Wayland input/permission flows are not implemented as a complete product. Use an
X11 session for initial desktop testing; unsupported backends return errors.

NSH uses GTK4. The following Debian/Ubuntu packages cover the first CI builds:

```sh
sudo apt-get update
sudo apt-get install -y pkg-config libclang-dev libxcb1-dev libxrandr-dev \
  libdbus-1-dev libpipewire-0.3-dev libgbm-dev libwayland-dev libegl-dev libgtk-4-dev \
  libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev
```

CI Linux archives are built on Ubuntu24.04 and require a compatible glibc (2.39 or newer); the headless archive is not a static musl build.
The native binary links system GTK libraries; it is not an AppImage and does not
bundle all desktop libraries. Headless hosts require a usable local shell and PTY.
Linux session permissions and DISPLAY/XAUTHORITY apply to capture and injection.

## macOS

Run the host in the logged-in user's session. Allow screen recording for capture
and Accessibility for input in System Settings. The operating system can deny a
feature despite successful compilation. Builds are initially unsigned and
unnotarized; release availability does not imply Gatekeeper notarization.

## Windows

Run the host in an interactive user session. Screen/input backends do not promise
UAC secure-desktop control, lock-screen control or session-0 service interaction.
PTY uses portable-pty's Windows backend. Distribution is initially unsigned.

## Operational behavior

Every session is attended through a one-time invitation. Closing the session
cancels PTY workers and incomplete uploads and releases tracked pressed input.
A host flag grants only its named service; the relay cannot turn services on.
Clipboard is accessed only through explicit actions. Transfer paths are relative
to the opened directory capability, and uploads refuse overwriting an existing
entry. No contents, keystrokes, tickets or invitation keys are logged.

Platform verification results belong in CAPABILITIES.md, with build and physical
runtime checks recorded separately. A synthetic screenshot used for protocol
testing is not evidence of platform capture.
