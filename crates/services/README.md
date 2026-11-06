# lightremote-services

Host service dispatch with no dependency on the session or mux actor. Every
permission defaults to disabled. File reads additionally require an explicit
share directory; file mutations require `allow_write`.

```rust,no_run
use lightremote_services::{HostConfig, HostServices};
use serde_json::json;
use std::path::PathBuf;

# async fn example(io: tokio::io::DuplexStream) -> anyhow::Result<()> {
let services = HostServices::new(HostConfig {
    share_dir: Some(PathBuf::from("/srv/shared")),
    allow_shell: true,
    allow_write: true,
    ..HostConfig::default()
})?;
services.serve("shell", json!({"stream_id": 42, "cols": 80, "rows": 24}), io).await?;
# Ok(()) }
```

`ServiceIo` is a public marker trait for `AsyncRead + AsyncWrite + Unpin + Send`,
blanket-implemented for matching types. `BoxServiceIo = Box<dyn ServiceIo>` is the
owned internal stream alias. The public interface is:

```rust,ignore
HostServices::new(config: HostConfig) -> anyhow::Result<HostServices>
HostServices::serve(&self, route: &str, metadata: Value, io: impl ServiceIo + 'static) -> impl Future<Output = anyhow::Result<()>>
HostServices::control(&self, request: Value) -> impl Future<Output = anyhow::Result<Value>>
HostServices::capabilities(&self) -> impl Future<Output = Value>
HostServices::subscribe(&self) -> tokio::sync::broadcast::Receiver<Value>
```

`HostServices` is cloneable and shares its limits, shell registry, input ownership,
clipboard owner, and share-directory capability between clones. Construct one
instance per authenticated host session so stream IDs and notifications are
scoped to that session. Dropping or aborting a `serve` future terminates its PTY
and releases its held input. The adapter must independently cancel the service
future on RESET/carrier failure, including after verified input FIN when a silent
child can leave the output drain without a pending transport I/O operation.
An independent mux-status cancellation watch should race the service future;
merely closing a raw pipe or relying on another read/write is insufficient. The transport must return verified remote FIN as
EOF and RESET/carrier failure as an I/O error; an untyped pipe closing cannot
establish successful FIN. Pass the mux's error-aware stream wrapper to `serve`,
not its raw byte-pipe field. Bounded `DuplexStream` inputs remain supported for
local tests and bridges that preserve those transport semantics.
A service error must become a visible stream RESET/error in the adapter.

`HostConfig` exposes `share_dir: Option<PathBuf>`, `allow_shell`, `allow_write`,
`allow_clipboard`, `allow_input`, `allow_desktop`, `shell: Option<PathBuf>`,
`max_file_size: u64` (1 GiB), `max_concurrent_streams: usize` (16, maximum 64),
and `io_timeout: Duration` (60 seconds per pending I/O/backpressure operation).
The shell override is configured locally; remote command metadata is rejected.

## Routes and control

- `control`, metadata `{}` or `null`: records are `u32be length || JSON`, at most
  1 MiB. Requests are `{id,method,params}`. Replies are `{id,result}` or
  `{id,error:{code,message}}`. Errors never include host filesystem paths.
- `shell`, metadata `{stream_id,cols:80,rows:24}`: raw bytes in both directions.
  The mux adapter must add its numeric stream ID (a `u32` fits the accepted
  `u64`). Only terminal dimensions 1–1000 are accepted. Verified input FIN
  drains accepted input, sends canonical terminal EOF when available, and keeps
  forwarding output until the child exits within `io_timeout`. Continuous output
  cannot extend that final drain deadline. RESET/read errors cancel immediately;
  raw-mode applications that do not finish before the deadline are terminated
  with an error. `shell.resize` uses `{stream_id,cols,rows}`. A real `shell.exit`
  notification reports `{stream_id,code,signal}` through control/subscribers.
- `file.get`, metadata `{path}`: raw bytes then FIN. The requester must FIN its
  request direction, and must send no data in that direction. Only regular
  files are supported. Downloads use the initial file length and reject early
  EOF or the configured size limit; concurrent external file mutation is not
  a consistent snapshot.
- `file.put`, metadata `{path,size}`: exactly `size` raw bytes and request FIN,
  then a framed `{size,sha256}` receipt and response FIN. Zero-byte uploads
  are valid. Partial, excess, timed-out, and cancelled uploads are removed.
  An atomic hard-link commit rejects an existing destination, including a
  concurrent creator. No fallback overwrites the destination when a filesystem
  lacks hard-link support. If the receipt cannot be delivered after commit,
  the complete destination remains; retries receive an existing-file error.
- `desktop`, metadata `{display:0,fps:10,quality:70,single:false}`: framed JPEG
  records, maximum 8 MiB each. `single:true` sends one JPEG then FIN. Display
  IDs are zero-based indices returned by `desktop.displays`; fps 1–30 and JPEG
  quality 1–100 are required. At most one captured frame is queued per stream.
- `input`, metadata `{display:0}`: framed JSON, maximum 64 KiB. Only one input
  stream is active per `HostServices`. Events: `{type:"move",x,y}`,
  `{type:"button",button:"left"|"right"|"middle",down}`,
  `{type:"scroll",dx,dy}`, `{type:"key",key,down}`, `{type:"text",text}`.
  Coordinates are local to the selected display; scrolling is limited to
  100 steps per axis, text to 16 KiB, and simultaneously held keys to 128.
  Key names accept single printable Unicode characters, Enter, Tab, Escape,
  Backspace, Delete, Insert, Home, End, PageUp, PageDown, ArrowUp/Down/Left/Right,
  Shift, Control/Ctrl, Alt, Meta/Super, Space, and F1–F12. Insert is available
  on Linux and Windows. Unknown fields,
  types, buttons, keys, coordinates, and oversized records are rejected.

Control methods are `capabilities`, `desktop.displays`, `shell.resize`,
`fs.list`, `fs.mkdir`, `fs.remove`, `clipboard.get`, and `clipboard.set`.
Filesystem params are `{path}`; list accepts `"."` for the share root and returns
`{entries:[{name,type,size}]}`. List is limited to 4096 scanned entries and a
512 KiB result. `fs.mkdir` creates one directory; `fs.remove` removes regular
files or empty directories without recursion. Clipboard set uses `{text}`;
get returns `{text}`. Clipboard text is limited to 256 KiB, is only read on an
explicit request, and its owner remains alive after setting it.

All paths are relative slash-separated UTF-8 paths. Absolute paths, `.`/`..`
components, empty components, Windows path syntax, and reserved staging names
are rejected. Every parent is opened with `cap-fs-ext::DirExt::open_dir_nofollow`
and the final download uses no-follow/nonblocking open plus a regular-file
check. This avoids both symlink escapes and check-then-open races. Capability
directory handles remain the authority; filesystem operation paths are never
joined to ambient host paths. A host process with existing privileged access
to the shared tree can still modify its ordinary files; this is not a sandbox
for a hostile local user running with the host's own credentials.

PTY directions each have four 16 KiB queue slots plus one in-flight chunk and
the caller's bounded duplex capacity. Blocking PTY read/write run on dedicated
threads. Backpressure does not block reading the opposite direction. Child
termination and reaping run on stream close or cancellation. On Unix the
saved PTY session ID and discovered descendants are terminated even when the
shell leader has already exited. Linux checks process generations and pins each
signal target with a pidfd; `/proc` and Linux 5.3+ pidfd support are required and
probed before remote input is accepted. macOS enumerates sessions through
libproc. Unix PTY handles are nonblocking: cancellation closes the idle reader
and a saturated writer without waiting for slave EOF. Canonical FIN delivery
waits until the foreground reader consumes the slave's queued input; writing the
bytes to the master alone is insufficient because readline terminal-mode changes
can discard a prematurely queued EOF. A temporary nonblocking slave handle
observes `FIONREAD` and closes immediately after delivery/cancellation so it does
not keep the PTY alive.

Session cleanup joins I/O workers and polls the shell's exit status within a
one-second deadline. An overdue child is handed to a single fixed-size reaper
which eventually collects its status without keeping a service stream permit or
creating another waiting thread. A host-wide limit of 64 live/pending PTY child
handles bounds this deferred state; `capabilities.limits.shell_processes` exposes
that limit. This is process-session cleanup for an explicitly permitted host-user
shell, not a containment boundary for commands deliberately daemonized into a
new session after becoming orphaned.
Control responses use an eight-element bounded queue, events a 64-element
broadcast ring; `events.lagged` explicitly reports dropped notifications.

## Builds and backend support

```sh
cargo test -p lightremote-services --no-default-features
cargo clippy -p lightremote-services --no-default-features --all-targets -- -D warnings
cargo test -p lightremote-services
cargo clippy -p lightremote-services --all-features --all-targets -- -D warnings
cargo doc -p lightremote-services --no-deps
```

The default `desktop` feature enables real xcap capture, enigo input, arboard
clipboard, and JPEG encoding. `--no-default-features` excludes all of them and
works without GUI development libraries. Desktop capabilities are probed from
real backends and explicit permissions; no synthetic screen or successful
input/clipboard no-op is returned when a backend is unavailable.

Linux capture uses xcap's X11/Wayland backends. This crate enables enigo's X11
backend, so native Wayland input injection is not claimed; XWayland availability
is determined by the real connection probe. Windows and macOS use the libraries'
native backends; those platforms require their own build/runtime verification.
macOS OS permissions must already be granted: the input probe does not open an
unsolicited permission prompt. Enumeration is not proof of permission to capture;
capture failures are returned to the adapter for a visible error.

Linux default builds need xcap's native build prerequisites, including libclang,
PipeWire, XCB, XRandR, DBus, Wayland, and EGL development files. See
[the verified sources](docs/source.md). If libclang is installed outside the
usual search paths, set `LIBCLANG_PATH` and (for a relocated compiler package)
`BINDGEN_EXTRA_CLANG_ARGS=-resource-dir=/path/to/lib/clang/<version>`.

The opt-in integration test below captures actual backend frames, decodes their
JPEG dimensions, checks a single-record screenshot FIN, and disconnects a live
stream. It deliberately fails if no display is reachable and does not substitute
fixtures. It was attempted in the development environment, where xcap exposed no
display; physical desktop runtime behavior remains to be checked on a host with
usable display/capture permissions.

```sh
cargo test -p lightremote-services --test services desktop_backend_emits_real_jpeg_single_fin_and_stream_disconnect -- --ignored
```
