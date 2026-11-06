# Dependency source verification

The implementation was checked against the published crates' own Rust source
and upstream documentation, rather than inferred from secondary tutorials.
Versions below identify the source inspected; the workspace lockfile determines
resolved dependencies. These are dependency references, not claims of having
run Windows/macOS physical desktop tests.

| Dependency | Primary source | API/behavior verified |
| --- | --- | --- |
| xcap 0.9.8 | https://docs.rs/xcap/0.9.8/xcap/struct.Monitor.html ; https://github.com/nashaofu/xcap ; published `src/monitor.rs`, `src/macos/impl_monitor.rs`, `README.md` | `Monitor::all`, monitor geometry, `capture_image() -> RgbaImage`; physical capture, upstream Linux prerequisite list and platform matrix |
| enigo 0.6.1 | https://docs.rs/enigo/0.6.1/enigo/ ; https://github.com/enigo-rs/enigo ; published `src/lib.rs`, `src/keycodes.rs`, `src/linux/mod.rs` | `Enigo::new(&Settings)`, `Mouse`/`Keyboard`, `Key::Unicode`, `Direction`, `Coordinate`, `Axis`; `release_keys_when_dropped`; permission prompt switch; `x11rb` feature |
| arboard 3.7.0 | https://docs.rs/arboard/3.7.0/arboard/struct.Clipboard.html ; https://github.com/1Password/arboard ; published `src/lib.rs`, platform implementation | `Clipboard::new`, `get_text`, `set_text`; Linux clipboard ownership lifetime; disable image dependencies with `default-features = false` |
| image 0.25 | https://docs.rs/image/0.25.10/image/codecs/jpeg/struct.JpegEncoder.html ; https://github.com/image-rs/image | `JpegEncoder::new_with_quality(...).encode_image(...)`, RGB conversion, JPEG writer errors propagate; JPEG-only feature |
| portable-pty 0.9.0 | https://docs.rs/portable-pty/0.9.0/portable_pty/ ; https://github.com/wez/wezterm/tree/main/pty ; published `src/lib.rs`, `src/unix.rs`, `src/cmdbuilder.rs` | native PTY creation, slave spawn, independent cloned reader/taken writer, resize, child kill/wait/process ID; Unix shell session creation with `setsid` |
| cap-std / cap-fs-ext 4.0.3 | https://docs.rs/cap-std/4.0.3/cap_std/fs/struct.Dir.html ; https://docs.rs/cap-fs-ext/4.0.3/cap_fs_ext/trait.DirExt.html ; https://github.com/bytecodealliance/cap-std | capability-relative directory operations, `open_dir_nofollow`, `OpenOptionsFollowExt::follow(No)`, nonblocking open, metadata, `Dir::hard_link` as no-overwrite commit; all child components anchored to handles |

The source files above were read from Cargo's published registry packages.
The default Linux feature build was also used to check the native dependency
closure: xcap pulls PipeWire bindings whose bindgen build requires usable
libclang **and its builtin C headers**. The headless dependency tree contains
none of xcap, enigo, arboard, or image.

PTY teardown additionally uses the kernel's saved session ID, process-parent
links, and process start generation. Primary interface references are
https://man7.org/linux/man-pages/man5/proc_pid_stat.5.html (including starttime
field 22), https://man7.org/linux/man-pages/man2/pidfd_open.2.html ,
https://man7.org/linux/man-pages/man2/pidfd_send_signal.2.html , and
https://man7.org/linux/man-pages/man2/fcntl.2.html . FFI signatures/constants were
checked in libc 0.2.191 source; macOS `proc_listallpids`, `proc_pidinfo`, and
`proc_bsdinfo` were checked in its Apple module. Nonblocking PTY I/O and saved-SID
teardown were exercised by real PTY tests, including 20 repeated one-slot
sessions for disconnect, abort, and an already-exited shell leader.
