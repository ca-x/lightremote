# Focused implementation review

Date: 2026-11-07. Status: three P1 findings fixed and independently verified for the recorded checkpoint; no remaining P0/P1 established within this review scope. All findings were sent immediately to the implementation lead. This report owns only this file, not `docs/review.md`.

Scope: the first staged snapshot in `/tmp/lightremote-services-relays-clients-review.patch`, current services, Rust/Cloudflare relays, and browser sources, against `docs/design.md` and the existing reviewer ledger. Application components, Rust edition 2024/MSRV 1.88. Native implementation and detailed core/CLI review are assigned separately. The v0.1 bearer-PSK threat model, absence of durable device identity/PFS, and one reliable ordered carrier are accepted scope decisions.

## Findings

### I1 — P1, fixed and independently verified: browser accepts final async processing as success after cancellation

Current fix locations: `web/src/link.ts:108` (`MuxStream.process()`), `web/src/client.ts:164` (`download()`), and its awaited sink-write / sink-close paths.

In the faulty snapshot, `process()` awaits the chunk handler and returns on `chunk.fin` without checking whether `fail()` ran during that await. If carrier loss or RESET happens while the final streaming file write is pending, `fail()` records an error but there is no further `read()` to observe it. `download()` then calls `sink.close()`, and the UI reports the transfer as successful. This violates the design requirement that RESET/carrier loss are errors.

Independent focused reproduction used the then-current faulty `MuxStream` and `download` source, transpiled with the installed TypeScript compiler inside a read-only, network-isolated Bubblewrap sandbox. It supplied a FIN-marked chunk, blocked `sink.write`, called `stream.fail(new Error("simulated carrier loss during final write"))`, then released the write. Observed output: `{"download":"resolved-success","sinkCalls":["write:3","close"]}`. `abort` was never called. No npm suite was rerun.

Resolution: the implementation now checks terminal failure after the awaited read, after each awaited handler, and after consuming the chunk. It retains FIN processing in stream lifecycle accounting. Independent focused checks against the revised actual source passed: RESET and carrier loss during the final sink write both rejected and called `abort()` without `close()`; a synchronous failure raised while returning credit also rejected before FIN success. This P1 is closed for the recorded source.

### I2 — P1, fixed and independently verified: PTY teardown does not terminate independent background process groups

Original finding locations: `crates/services/src/shell.rs:239` (`Session::stop()`), `crates/services/src/shell.rs:244` (leader-status guard), and `crates/services/src/shell.rs:200` (blocking reader retains a service permit), in source SHA-256 `786ad4236788bbae0ebe9057be39040a138f8fc5bad88b33dca9437d96605af2`. Those sections have since been rewritten; current cleanup lives in `Session::stop()`, `Cleanup::run()`, and `shell/{unix,reap}.rs`.

In the faulty snapshot, Unix cleanup signals only the PTY foreground process group and the shell process group. A job-controlled background process can have a different process group and retain the PTY slave. A background job inheriting ignored SIGHUP survives both signals. Additionally, all group cleanup is skipped when `child.try_wait()` already reports that the shell exited. The reader is already blocked in `read()` and cannot observe `running = false`; retaining the service permit in that worker bounds the leak by permanently consuming service capacity. Repeated shell sessions can exhaust the default 16 service slots and prevent further services from opening.

Reproduction input in the real interactive PTY: `set -m; trap '' HUP; sleep 600 & printf '\nBG=%s SHELL=%s\n' "$!" "$$"`, followed by peer drop or service cancellation. A second variant sends `exit` after the marker. The services owner independently reported two failing real-service regression tests: the separate-PGID background child survived in both the live-leader and already-exited-leader cases. The failing pre-fix Rust runs were owner-reported; this reviewer independently executed the revised targeted tests below.

Resolution: teardown now covers the retained Unix session/descendant tree even after shell exit, verifies process generation, uses Linux pidfds to pin signal recipients, makes PTY I/O nonblocking and cancellable, and bounds worker joins. A separately bounded 64-child supervisor retains overdue wait handles without retaining the service slot. Current fix locations: `crates/services/src/shell.rs:233`, `crates/services/src/shell.rs:320`, `crates/services/src/shell/unix.rs:12`, `crates/services/src/shell/unix.rs:73`, and `crates/services/src/shell/reap.rs:30`.

Independent verification used the owner's newly built test executables after checking that the isolated build-copy shell sources matched the shared workspace byte for byte:

- `/tmp/lightremote-services-check/target/debug/deps/services-f9c8ce7707fa1bcd background_ --test-threads=1 --nocapture`: **3 passed**, 0 failed, 2.62 seconds. Covers peer disconnect, already-exited shell leader, and service abort, each repeated 20 times with a single service slot. All 60 cycles check background termination and service capacity recovery.
- `/tmp/lightremote-services-check/target/debug/deps/lightremote_services-6e6b7ee707ec69a5 shell::tests --test-threads=1 --nocapture`: **2 passed**, 0 failed, 1.05 seconds. Covers cancellation with a deliberately retained slave and saturated writer, plus deferred reaping without retaining the service permit.

Both commands ran in fresh isolated sandboxes; no shared cargo build was launched. This P1 is closed for the recorded Linux source and executable checkpoint. These tests do not establish macOS/Windows runtime behavior.

### I3 — P1, fixed and independently verified: browser treats graceful GOAWAY as immediate failure of accepted streams

Current fix locations: `web/src/link.ts:363` (`Link.event()`), `web/src/link.ts:415` (`checkDrain()`), and `web/src/link.ts:322` (refuse new OPEN during drain).

In the faulty snapshot, the GOAWAY event immediately calls `close()`, which fails every accepted stream and clears queued outbound packets. A host that sends GOAWAY while a previously accepted download or upload is still completing therefore loses that operation instead of allowing it to drain. The application contract explicitly requires GOAWAY to refuse new streams and drain accepted streams. This is the browser lifecycle path, separate from the core writer-drain finding in the existing ledger.

Resolution: the implementation now retains accepted streams, refuses new opens, enforces a finite drain deadline, and places an end-to-end PING/PONG barrier after final ordered packets before closing. A focused independent check loaded the actual complete revised Link/download/record sources and pinned released Node SDK Session/framing in the isolated sandbox. It negotiated a session, accepted file.get, delivered a real host GOAWAY during that transfer, and blocked the final sink write. The connection and stream remained alive and a later OPEN was refused. Releasing the handler completed and committed the download; the PING/PONG barrier was acknowledged before socket close. The carrier in this check was an in-memory identity carrier; it did not test WebSocket networking or crypto. This P1 is closed for the recorded source.

## Evidence and review limits

No additional concrete P0/P1 was established in the reviewed Rust/Cloudflare relays, file sandbox/atomic upload paths, desktop/input cleanup, browser crypto parity, or capability/error presentation. This is a scoped source-review result, not a blanket correctness or security pass.

Relay inspection covered ticket reservation and replay, disconnect/failed-upgrade teardown, expiry/wait timers, queue/write bounds, singleton DO allocation, durable attachment/join reconciliation and busy markers, chunk persistence/deletion, and cumulative byte/record caps. Browser inspection covered Rust-compatible salt/info/AAD/counter construction and the shared fixture, record assembly and WINDOW progress, upload receipts, download errors, and visible host capability errors. The fixture and existing tests were read; their full suites were not rerun.

Only focused checks for the three concrete findings were executed. They ran with read-only source/toolchain mounts, no external network, an empty allowlisted environment, ephemeral writable storage, 10-second CPU/15-second wall limits for browser checks and 20-second CPU/30-second wall limits for PTY checks, 1 GiB virtual memory (64 MiB JS heap for browser checks), and a process allowance bounded relative to the host baseline. Two sandbox-launch attempts encountered the host process limit before target execution; the subsequent focused runs completed successfully. No existing passing npm suite was rerun. No subagents or commits were created and only this report was written.

A targeted Nowledge search was performed; no retrieved historical decision superseded the supplied contract. The memory graph was skipped because this CLI context could not establish the browser identity and scope required by the memory skill.

Source changed concurrently. The following SHA-256 values record source snapshots and test executables observed during this review; they are deliberately not refreshed to unreviewed later working-tree edits. Test executable names can be reused by subsequent builds, so their recorded hashes identify the observed build artifacts. The services owner has announced subsequent generic ServiceIo and shell normal-FIN changes; those are outside this checkpoint and require their assigned follow-up review. No native/core approval, physical desktop validation, live Cloudflare deployment, or release approval is implied.

| Recorded file / artifact | SHA-256 |
| --- | --- |
| `/tmp/lightremote-services-relays-clients-review.patch` | `95340c3715180f2d5a77d07c9864320f577702f7e2af101eb2a03c363851faf5` |
| `docs/design.md` | `d9ae663710628f7daf1944ad8babec284e1a3e7a0516088e0b692a2fb4badd77` |
| `crates/services/src/desktop.rs` | `cc0461e736f866fc351d6b9ac0155d878830205a90b3302f903681257ca8e32f` |
| `crates/services/src/files.rs` | `f9de7c79a544bb394ddda3ff0293e1818d76db6418ce7b7ebbda6651c66d2a0c` |
| `crates/services/src/lib.rs` | `00fb1a0ddea34921469eeb4ea31e7c31ded159ee073cad0088ec6cb2ebac4882` |
| `crates/services/src/records.rs` | `1e0d6844027f797ec72d1328de6944d895f1aceea9ffc73d8ae5e9b95b6716fd` |
| `crates/services/src/shell.rs` | `c5ab0571e2c1bbecaedeb19a14a88c119062079d23759d9054d3808e15ed79c3` |
| `crates/services/src/shell/unix.rs` | `e036d91202945e54b325fe7634f835f7d267bacab884994d9e35eb4868f81897` |
| `crates/services/src/shell/reap.rs` | `486c634e66b16c5ce00c032d1e3268aaeb6bb1cbeab60bcca4fea0246a875774` |
| `crates/services/tests/services.rs` | `8b51580b9b0de5e7dd290d164b07962abdd2490ce765365edfef7d3cb97d4048` |
| `crates/relay/src/lib.rs` | `79fdc1f2000c19e003e168c550befc430ed8d2f25a654a2aa11d292a8095cb5c` |
| `cloudflare/src/index.ts` | `a6b2422454112a067f78a4bfbf8e51d91187a32b43f5c1a1c56428c9932a589b` |
| `web/src/client.ts` | `c0766cbcca52dfbe6977a48b454ebdd5a719dd62865de9619ad1562090040625` |
| `web/src/link.ts` | `87594b2b132918daa8d0e0c964be7bc3998e82bc2467c1cf403b122dbcf134f5` |
| `web/src/main.ts` | `6d788dffe2e6ab8afbeb0712c97a08b55557c82c60e0e17de5d0c72b7e804048` |
| `web/src/records.ts` | `0f53295a379c3120054fb4e20fe0e042c44254ef5fe3e683b66d0266b4c71ed5` |
| `web/src/secure.ts` | `66084681d8147db9c9f36b0f9c77eb8c2bad5e706c08d5a260eeb5aa3b2c6b99` |
| `/tmp/lightremote-services-check/target/debug/deps/services-f9c8ce7707fa1bcd` | `76fbefea9103c6cb77596b52539cb0bbdf3d5b52a2b38702c58a7e7c862f669f` |
| `/tmp/lightremote-services-check/target/debug/deps/lightremote_services-6e6b7ee707ec69a5` | `b8d1defc041230d4f53f870d2ccfa138d81b7dd297fbee568a33d9127f525f6f` |
