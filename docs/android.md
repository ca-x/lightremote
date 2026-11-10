# Experimental Android viewer

The Android entry is a Rust NSH viewer, not a mobile host or the full desktop
controller. It uses the same encrypted client and the same pinned NSH revision
`0e4a20bd41ee72a35515cecbf9a7fba88742d911`. No Flutter, WebView, RustDesk code or
uu-remote-linux code is included.

## Scope

- Connect with an explicit invitation and clear the invitation field on submit.
- Start and stop read-only viewing of host display 0; refresh host permissions.
- Native page navigation, 44-point controls, a single scroll owner, safe-area
  insets and software-keyboard avoidance.
- No input injection, touch-to-mouse, automatic capture, clipboard, terminal,
  local file transfer or background host service. Host permissions do not make
  an unimplemented mobile feature available.

The manifest declares INTERNET and the NSH runtime's POST_NOTIFICATIONS
permission. This viewer does not request notification consent or send
notifications. It declares no screen-capture, accessibility-service or storage
permission. Android system consent is separate from remote host grants.

## Build

Use the NSH checkout matching the pinned revision, not an unversioned installed
CLI. Set `ANDROID_HOME` to an Android SDK containing platform 35, build-tools
35.0.1 and NDK 27.3.13750724; Java and a usable libclang are also required.
The runtime uses minimum SDK 28 (Android 9), not SDK 26.

```sh
export ANDROID_HOME=/path/to/android-sdk
export ANDROID_NDK_HOME="$ANDROID_HOME/ndk/27.3.13750724"
export CLANG_PATH="$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64/bin/clang"
# If libclang is not installed in a standard location:
export LIBCLANG_PATH=/path/to/libclang
rustup target add aarch64-linux-android

# NSH_SRC is the checkout of the pinned revision above.
cargo build --locked --manifest-path "$NSH_SRC/Cargo.toml" -p nsh-cli
(cd "$NSH_SRC" && cargo xtask android --abi arm64-v8a)
"$NSH_SRC/target/debug/nsh" package crates/native \
  --target android --abi arm64-v8a -o dist/android
```

Run the last command from this repository. Keep NSH's default `target` location
when running its xtask: the pinned xtask locates its output there even when
Cargo accepts a different `CARGO_TARGET_DIR`. When using a separately installed
NSH distribution, provide its matching prebuilt Android runtime instead.
`CLANG_PATH` selects the NDK resource headers for bindgen; host GCC headers must
not be used as Android standard headers. No Gradle installation is needed.

Without an explicit release keystore NSH signs with an **Android Debug** key.
That is suitable only for local testing. Do not describe this APK as a signed
production/store release, distribute the debug key, or replace a production
installation with it. Desktop release archives remain unsigned and separate.

## Evidence and remaining checks

On 2026-11-10 the real ARM64 APK was built using the pinned NSH CLI and runtime,
SDK 35, NDK r27d and OpenJDK 27. `aapt` confirmed package
`io.github.cax.roc_remote`, version `0.1.0`, minimum SDK 28, target SDK 35,
launcher activity and product name. `apksigner verify` confirmed v3 signing
with an Android Debug certificate. `zipalign -c -P 16 -v 4` passed; the actual
stripped AArch64 `libnsh.so` has 16-KiB-aligned LOAD segments. The owner-supplied
logo supplies the launcher icon.

Headless UI tests cover the explicit preview boundary, disabled disconnected
actions, invitation input and a 360×780 viewport with large text, safe areas and
a software keyboard, in light and dark mode. This does **not** establish actual
Android rendering, IME behavior, image updates, rotation, lifecycle/reconnection,
TLS/network behavior or successful installation on a physical device. No
emulator/device run, store submission or release-key signing is claimed.

The locally built APK is a test artifact, not part of the hosted desktop release
asset/checksum matrix. Before mobile release, install it on a real Android 9+
device, complete an encrypted viewing session, test background/resume and
rotation, and establish a protected release keystore and reproducible CI build.
