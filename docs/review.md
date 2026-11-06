# Independent review ledger

<!-- scoped-burst-review-start -->
## Current scoped decision: 0.1.1 receive-burst fixes

**Source review: PASS for C12 (Rust) and C13 (Web) at the three frozen hashes below. No unresolved P0/P1 was found in this patch scope. The receive-burst runtime incident is CLOSED on implementation-lead-executed browser stress evidence inspected by this reviewer.** Scoped source and runtime acceptance is complete; hosted 0.1.1 CI/release acceptance is not claimed.

Scope: the pending-input delta and two new regressions in `crates/core/src/link.rs`, the receive-buffer delta in `web/src/link.ts`, and the three tests in `web/tests/receive-burst.test.ts`. Related read/consume/retire/reset paths were inspected to check their contracts. Services, native, relay, version changes and unrelated lifecycle work are outside this delta review. Existing C2/C3/C4/C7/C11 dispositions remain tied to their historical checkpoints.

### Findings and accepted fixes

- **C12, P1: valid Rust input rejected by packet count.** The implementation lead's packaged-binary reproduction during tag run [42288162779](https://github.com/lib-x/lightremote/actions/runs/42288162779) reached five received bytes and zero consumed bytes; the old four-slot input channel caused RESET. Current `link.rs:241` coalesces one pending suffix within the local receive limit; `:446` restores the exact record on Full instead of resetting. New bytes append after the queued prefix. Retry occurs on DATA, Consume and pump. FIN closes the sender only after its final record is enqueued; retirement also requires no pending suffix (`:487`). These changes address the reproduced source defect.
- **C13, P1: equivalent Web receive packet-count rejection.** The old 256-chunk threshold rejected legitimate 4096/1024-byte bursts within 128KiB credit. Current `web/src/link.ts:133` enforces outstanding bytes and combines only an undispatched queued tail. `read()` removes a delivered chunk from that queue, so later input cannot change its bytes or FIN. The consume closure uses the final chunk length/FIN and executes once; failed streams ignore receive/consume. This addresses the sibling source defect without removing outbound admission limits.

### Bounds, ordering and cleanup assessment

Rust Session still enforces stream and aggregate connection receive credit before application events. The added limit is the smaller local stream/connection window (production default 64KiB); geometric pending growth requests no more than that limit. The four-slot channel remains bounded. Logical unconsumed payload is bounded by protocol credit; vector capacity, channel metadata and duplex buffering add bounded overhead, so credit is not an exact process-memory measurement. Consumption still follows successful bridge acceptance. Full preserves FIFO; empty and payload-bearing FIN cannot overtake pending data. RESET removes the pending suffix, aborts bridge workers and leaves the existing failed-stream read boundary intact. Late Consume finds no stream and cannot refund credit twice.

Web outstanding includes delivered-but-unconsumed bytes and the queued suffix. Each new tail allocation is capped at 128KiB; growth may temporarily retain the old allocation. The normal `process()` path has one delivered chunk and one queued suffix, and the SDK also enforces aggregate connection credit. FIN is consumed only with its final chunk; retirement requires consumed FIN and zero outstanding bytes. Failure clears queued input and makes previously delivered consume closures inert. No low-latency or exact-RSS claim follows from these bounds.

### Execution evidence and incident closure

- **Rust, Boole-reported:** old source fails at byte five. Frozen-source regression feeds 65,536 patterned single-byte packets before bridge scheduling, with both FIN forms, and checks exact FIFO output, pending size/capacity, completion and no RESET. The additional cleanup regression covers pending DATA with/without FIN, worker termination, exactly 32 bytes of returned connection credit, ignored late Consume and full-window reuse by another stream. Both regression sources were independently inspected (`link.rs:1050`, `:1163`).
- **Rust, Boole-reported:** `cargo test --locked -p lightremote-core` passes 11 unit tests plus one integration; the headless CLI `reset_upload` integration passes; core Clippy with warnings denied, docs and formatting pass. No dedicated execution log was saved; provenance is Boole's task report, not an independent test run by this reviewer.
- **Web:** the reviewer inspected `/tmp/lightremote-web-burst-before.log`: both legitimate-burst cases fail on old source; the byte-window enforcement case passes. The implementation lead reports frozen-source typecheck, all 37 tests and production build passing. The three new test sources were independently inspected: exact bytes/FIN, unchanged in-flight chunk with idempotent consume, and rejection above 128KiB.
- **Runtime incident CLOSED:** the implementation lead reports ten consecutive executions of unmodified `web/scripts/rust-smoke.ts` against rebuilt 0.1.1 CLI/relay binaries and the production Web bundle, with no added delays or retry changes. The reviewer inspected `/tmp/lightremote-0.1.1-browser-stress.json`: iterations 1–10 each record exit 0, the frozen core/Web source hashes, binary hashes and a distinct log path. All ten referenced logs were read and contain the real Chromium/Rust encrypted-TMP smoke PASS covering PTY output, resize, upload/download and the explicit headless-desktop error. The smoke script has no worktree diff. Execution owner is the implementation lead; this reviewer inspected the artifacts and did not rerun the browser. This satisfies the scoped runtime gate for the reproduced incident.
- **Current local full validation:** implementation lead reports `bash scripts/check.sh` exit 0. The reviewer inspected `/tmp/lightremote-0.1.1-full-check.log` and the script: 45 headless Rust tests pass; 47 desktop/default Rust tests pass with one live-capture test explicitly ignored; Web typecheck, 37 tests and production build complete; Cloudflare typecheck and 10 tests complete. Core's 11 unit tests plus one integration pass in both Rust profiles. `/tmp/lightremote-0.1.1-native-tests.log` separately records 16 passing native tests. These are current local execution results, not physical-desktop or hosted-release acceptance.
- **Separate release status:** hosted 0.1.1 CI, tag/release jobs, published assets, signing and physical-platform acceptance are outside this closure. No hosted-release pass is claimed. The earlier frozen report's CI observations remain historical.

No broad tests were rerun by this reviewer, as requested. Source hashes and `git diff --check` were checked locally. Only this ledger was edited. The prior frozen report below is preserved unchanged and describes earlier checkpoints, including its then-current CI observations.

### Execution artifact checkpoint

Execution owner: implementation lead. Artifact inspection and source-hash comparison: this reviewer. The stress manifest records CLI SHA-256 `f10ab683067e4037bd1362c4bcad572e45d2f6173bb78c2a5911601696fc3f45` and relay SHA-256 `ebbe8c51d9fc60a245b1963e21c38f397abc6e46fdba2c4a2cdef44025150429`; these are manifest provenance, not a separate binary rebuild by this reviewer.

| Inspected artifact | SHA-256 |
| --- | --- |
| `/tmp/lightremote-0.1.1-browser-stress.json` | `fade73d239abf3d10cb1c43d57412e9f50004840fd93f7094211bf9051879e33` |
| `/tmp/lightremote-0.1.1-full-check.log` | `cbab63119c0a9fdb94bc85ef05d434bd860817e70ae63397f8049f1850a969fd` |
| `/tmp/lightremote-0.1.1-native-tests.log` | `e7398b607dcc365c0b3dd852c1f968f2592f208930af67345a4fb440872b95ed` |

The ten browser logs are `/tmp/lightremote-0.1.1-browser-{1..10}.log`; each contains the same 139-byte PASS output, SHA-256 `f14d0c1661bda0e99254f144e986cade28d6e10b7dd166de6bc5f1d697fb5e21`. Individual exit statuses come from the stress manifest, not from inferred success text alone.

Report frozen for the 0.1.1 commit/tag handoff after this evidence update. C12/C13 source fixes and the scoped runtime incident are closed. TMP 0.1.0 and protocol scope are unchanged. No source edits, broad test reruns, commit, tag, push or publication were performed by this reviewer.

### Frozen patch source checkpoint

Worktree baseline: `7653671ce0ee68f7eec6a347d8ce1fec5a9e335c`; reviewed patches are uncommitted at this checkpoint. All three hashes were independently read and match their owners' freeze reports.

| File | SHA-256 |
| --- | --- |
| `crates/core/src/link.rs` | `43c476604462071509f4636c41096a94c828bbcea2636141fd554942952accfe` |
| `web/src/link.ts` | `9844a6b84954d381567e7e437cac372a3b2cc44f291892c364886a47e2c571cb` |
| `web/tests/receive-burst.test.ts` | `8dea3effaf1a9cf905d80f7d6505c7b7674d4f5b0a0ff5b8fd7eb24336755178` |

Changes after these hashes require a delta review. The 0.1.1 implementation scope is an implementation-lead decision under delegated design authority. The immutable v0.1.0 tag is not replaced by this report.
<!-- scoped-burst-review-end -->

## Earlier frozen review (unchanged)

## Latest decision

Post-freeze execution evidence: hosted CI run `42285530612` completed successfully
for source `3d0aa0b`, including all five platform builds, Docker smoke and real
browser/Rust/Workers integration. This evidence does not broaden the scoped
source-review sign-off below.

**Scoped final review: PASS for C2, C3, C4, C7 and C11. These five findings are closed for the exact source checkpoint below. No unresolved P0/P1 was found in these reviewed paths.**

**Local final validation: PASS for `bash scripts/check.sh`. Exit 0 is reported by the implementation lead; the final output log was inspected by this reviewer. Hosted CI is [in progress](https://github.com/lib-x/lightremote/actions/runs/42284398244) for published commit `49ef5ea` (status checked through GitHub Actions).**

Basis: independent source re-review, inspected regression code, implementation-lead-reported test execution, and inspection of the final local check log. This reviewer did not independently rerun target tests. The five-finding sign-off and the successful local script are recorded separately from hosted CI, native/physical-platform acceptance and publication.

| Finding | Final scoped disposition | Source and regression evidence |
| --- | --- | --- |
| C2 scheduler starvation | Closed | core link.rs:616-644 rotates after the last successful send using a persistent cursor. fair_credit_rotation_and_unclaimed_open_cleanup checks one-chunk connection-credit rotation with the first stream refilled. Targeted pass reported by implementation lead. |
| C3 abandoned pending OPEN | Closed | core link.rs:600-614 resets opening replies whose receivers closed; independent one-second maintenance calls it at 840-841. The cleanup unit case confirms removal. Targeted pass reported by implementation lead. |
| C4 remote HTTP invitation | Closed | invite.rs:89-103 requires HTTPS outside exact parsed loopback hosts and rejects URL credentials. invitation_requires_authentic_client_delivery covers rejection and valid HTTPS/loopback cases. Unit pass reported by implementation lead. |
| C7 upload RESET treated as FIN | Closed | MuxStream's AsyncRead separates failed status, missing-FIN EOF and actual received FIN; CLI passes the whole stream; ServiceIo boxing preserves that boundary. Actual encrypted reset_upload integration covers staged zero-byte/128KiB exact-size requests, RESET without FIN, no commit, staging cleanup and valid empty-FIN receipt on the same connection. Integration pass reported by implementation lead. |
| C11 queued writes hidden by bothFIN | Closed | core link.rs:264-291 increments critical-write count before publication and rolls it back on enqueue failure; writer:766-774 decrements only after successful send/flush. carrier_fully_written:595-598 requires count zero, ready and both FIN flags. empty_stream_table_cannot_hide_unwritten_data checks the previous empty-map counterexample. Unit pass reported by implementation lead. |

The reported C7 command is `cargo test -p lightremote --no-default-features --test reset_upload`. The test's source at cli/tests/reset_upload.rs:40-89 waits until the staging file has the declared size before RESET, then asserts service error, absent destination and no staging leak. Lines 91-113 verify subsequent successful work on the same encrypted connection. It therefore exercises the original failure path rather than merely cancelling an unused stream. It invokes the real host service adapter directly; CLI dispatch was checked separately at main.rs:161.

## Validation status and remaining scope

- **Relay/tiny-window evidence provided:** relay owner reports final PASS after the fair one-pending/backpressure patch, including current core 9 unit tests plus 1 integration test and the 512KiB tiny-window/GOAWAY case. This reviewer did not independently retest or re-review the latest relay patch. The provided desktop log separately confirms the named core test and relay test outcomes.
- **Local full check script: PASS.** The final `bash scripts/check.sh` run completed with exit 0 as reported by the implementation lead, and its log was inspected. Both headless and default/desktop Rust profiles pass, including canonical shell input FIN. The previous headless full-suite execution pending item is closed by this run. Hosted CI is [in progress](https://github.com/lib-x/lightremote/actions/runs/42284398244) for `49ef5ea`; no hosted-CI success is claimed.
- **Other observations:** C1 FIFO/Finish and C9 decoder.finish fixes were inspected in earlier checkpoints. Heartbeat scheduling at 840-847 and the 60-second drain/PONG bound at 854-856 are present in the current core. C8 input-FIN/output-drain source changes were inspected separately; full PTY/process and platform evidence is recorded by that review owner. This scoped decision does not silently approve those independent work areas.
- **Product/release evidence:** web/native, physical desktops, Cloudflare deployment and signing retain their own statuses. A missing unrelated execution report does not turn C2/C3/C4/C7/C11 back into source defects.

This reviewer owns this file and changed only this ledger. Scope choices were implementation-lead decisions under delegated design authority, not explicit user quotations. Native and reference-project status is not inferred from this five-finding review.

## Final local execution evidence and report freeze

- Command: `bash scripts/check.sh`; execution owner: implementation lead; reported exit status: 0. This reviewer inspected the script and supplied output, rather than independently rerunning target code.
- Final artifact: `/tmp/lightremote-final-checks-3.log`, SHA-256 `0b7535074a6ef6945087c19e5e72b9a95ae35f39e9549a613e93eede41df5d07`.
- Script inspected at freeze: `scripts/check.sh`, SHA-256 `ad74f09340c5df3b93ed2c36c4e70c80074949584acacde8732e6f260c79ef49`. It uses `set -euo pipefail` and checks Rust formatting, headless/default-profile Clippy with warnings denied, Rust tests, Web typecheck/tests/build and Cloudflare typecheck/tests.
- Rust log: both profiles report core 9 unit tests, core roundtrip integration 1, encrypted RESET-upload integration 1 and relay 10 passing. Headless services report 2 unit plus 20 integration tests passing. Default/desktop services report 5 unit plus 19 integration tests passing, with 1 physical-capture test explicitly ignored. Doc tests finish successfully in both profiles.
- The previously flaky canonical shell input-FIN case passes in both profile sections of this final log. The earlier headless execution failure is historical; no local full-suite execution is left pending on that basis.
- Web log: typecheck succeeds, 34 tests pass with 0 failures, and the production build completes. Cloudflare log: typecheck succeeds and 10 tests pass. The log reaches the script's final Cloudflare test stage successfully.
- Scope of this script: Rust uses Cargo default-members, not the whole workspace/native member; Web tests are the configured Node test suite. Hosted CI, native-platform builds, browser/physical-desktop acceptance and live deployment retain their own evidence. Local script success is not recorded as completion of those separate activities.
- Earlier desktop evidence remains traceable at `/tmp/lightremote-desktop-tests.log`, SHA-256 `2ff9040b88d65002a1a4986c49d40b003c87e439852111ebcd0e6fa177bdd466`; it is superseded by the final local run for execution status.
- Hosted CI evidence: initial source commit `49ef5ea34f59e8ebbc1a365d0674dc81c227f97d` is pushed. [Actions run 42284398244](https://github.com/lib-x/lightremote/actions/runs/42284398244) was read via `gh run view` and returned `status=in_progress`, an empty conclusion, and that exact head SHA. Local HEAD matches. This is a frozen status observation, not a final CI result.
- Freeze: report frozen after this explicitly requested hosted-CI status update. Scoped findings C2/C3/C4/C7/C11 remain closed. Hosted CI has no final verdict at this checkpoint. No source edits, target test execution, commit/push or publication was performed by this reviewer.

## Final scoped source checkpoint

| File | SHA-256 |
| --- | --- |
| `crates/core/src/link.rs` | `2bbf6bb1618496b6dd7a749ca542b8fd8b654577d407ed5fba177043b4169d3f` |
| `crates/core/src/invite.rs` | `ac4058dc8d5e86371c8a75e659726d991bd1972321536f3f47b6b27fe9a3c05e` |
| `crates/cli/src/main.rs` | `1180e75f0b320230ef564a2996e751434e383407b604a4e01048b606a0c2e7b4` |
| `crates/cli/tests/reset_upload.rs` | `1c669a2f98f83c44790955b20d04c2302dc5e1b9ae531be5381271c00592229a` |
| `crates/services/src/lib.rs` | `8bbcbb79dfe112bbd4207aa84d1d9e1d07264d03e4389c87bf7f81549494ff91` |
| `crates/services/src/files.rs` | `53abab9795e5c1b278a00175a0d960808b228af4a902af4926d279b64b03e4aa` |
| `crates/services/src/records.rs` | `1e0d6844027f797ec72d1328de6944d895f1aceea9ffc73d8ae5e9b95b6716fd` |

Changes after these hashes need a delta recheck; formatting-only changes may be recorded as such after comparison. The review's test-execution provenance remains implementation-lead-reported even when the same source is later formatted.

## Historical review record

All entries below describe the explicitly recorded earlier source hashes and review times. Their “open”, “P1”, “pending” and counts are historical assessments, superseded by Latest status above. Preserve them for traceability, not as the current publication checklist.

<details>
<summary>Earlier design and implementation checkpoints, including superseded findings</summary>

### Superseded latest summary before final scoped sign-off

#### Latest status

**Source fixes inspected; execution sign-off pending.** Previously reported P1s must not be presented as current unfixed defects merely because historical entries below say “open”. This checkpoint establishes the source changes listed here, not an implementation, security, platform or release pass.

This reviewer owns this file. Implementation and reference sources were inspected read-only. Scope decisions were made by the implementation lead under delegated design authority, not quoted as explicit user requirements. The accepted attended invitation-PSK scope, trusted client origin, experimental single-link JPEG and absence of persistent identity/PFS claims remain unchanged.

| Item | Latest disposition | Remaining evidence |
| --- | --- | --- |
| C1 core drain/FIFO | Source fix inspected: terminal reader events follow packets; writer Finish flushes the ordered output; peer-PONG drain barrier and deadline are present. | Exact current drain-test execution result. |
| C2 / C3 / C11 | Source fixes inspected: rotating cursor, maintenance cleanup of abandoned pending OPENs, and critical-write counter. No remaining P0/P1 was found in the prior three-item scoped re-review. | Test result tied to the reviewed source/delta. |
| C4 client URL delivery | Source fix inspected in invite.rs:89-103. External HTTP and URL credentials are rejected; exact configured loopback hosts retain the HTTP development exception. Test at 128-140 covers rejection, HTTPS, parsing and IPv4/IPv6 loopback. | Current test execution result. |
| C5 drain/liveness | 60-second stream/PONG drain bound inspected; final writer Close has its own timeout. | Ongoing idle-heartbeat behavior was not re-audited in this checkpoint. |
| C6 controller terminal status | completed() remains available; the earlier missing-status API finding is historical. | Consumer-specific boundary tests and review. |
| C7 upload FIN versus RESET | Source fix inspected end to end through the host adapter: error-aware MuxStream read, independent received-FIN flag, whole-stream CLI dispatch and boxed ServiceIo. The original raw-Duplex host commit path is superseded. | Real zero-byte and exact-size RESET-without-FIN upload regressions; no destination/staging leak, unrelated stream survives. |
| C8 shell input FIN | Source fix inspected: normal input EOF stops the input pump but waits for output under a deadline; read errors go to cleanup. Unix input worker drains accepted bytes before canonical EOF. The old immediate-stop-on-normal-FIN finding is historical. | Delayed real PTY output, child exit/reap and RESET cancellation results. |
| C9 close truncation | Source fix inspected at core link.rs:794-801: both explicit WebSocket Close and socket EOF call decoder.finish before CarrierClosed classification. | Partial TMP frame plus Close rejection test/result. |
| C10 relay queue closing | Relay-owner changes are delegated; the relay was not re-reviewed in this checkpoint. Core end-to-end prefix barrier mitigates the previously identified normal-close trigger. | Relay-specific source recheck and black-box closing/fairness evidence. |

**Current evidence state:** the implementation lead reports prior core/CLI smoke success and shared Rust/Node vector agreement. This reviewer has inspected test sources and the modifications above, but has not independently executed current target tests. The upload RESET integration is reported as next work. No pending execution is silently marked passed, and no fixed source item is left labeled an unfixed P1 in this summary. Native client, physical desktops and full release readiness retain their separate evidence requirements.

#### Current source checkpoint

| File | SHA-256 |
| --- | --- |
| `crates/core/src/link.rs` | `78049444ea1ebcf16f386a76e9487441c28782d103513fbdc1915563b382e6b7` |
| `crates/core/src/invite.rs` | `ac4058dc8d5e86371c8a75e659726d991bd1972321536f3f47b6b27fe9a3c05e` |
| `crates/cli/src/main.rs` | `1180e75f0b320230ef564a2996e751434e383407b604a4e01048b606a0c2e7b4` |
| `crates/services/src/lib.rs` | `66f660095659583ddcb624ea537341093c7c840f90ce7e8b2e202eeb384cde3c` |
| `crates/services/src/files.rs` | `53abab9795e5c1b278a00175a0d960808b228af4a902af4926d279b64b03e4aa` |
| `crates/services/src/records.rs` | `1e0d6844027f797ec72d1328de6944d895f1aceea9ffc73d8ae5e9b95b6716fd` |
| `crates/services/src/shell.rs` | `02312c17defd194296a5f44dbee7cf8957cc544c73ef5ba1967edcc444b86569` |

C7 details in this snapshot:

- core link.rs:73-99 returns ConnectionReset for failed status and UnexpectedEof for bridge EOF without an authenticated received FIN. A zero-length read is exempt from the EOF check.
- receive_fin is stored only in Event::Data with fin=true at 475-480. It does not depend on local FIN or completed().
- core link.rs:991-1012 has a unit case distinguishing normal FIN, failed status and an unmarked broken bridge.
- CLI main.rs:161 passes incoming.stream, retaining the wrapper. services lib.rs:175-181 accepts ServiceIo and boxes that object without extracting raw DuplexStream; files.rs receives BoxServiceIo. The service can therefore reject RESET before commit while emitting its receipt afterwards. This avoids the commit/completed() wait cycle.
- This does not claim atomic cancellation of a filesystem commit after a genuine FIN has already authorized it. The integration must define and test that race boundary.

C8 details in this snapshot: services shell.rs:109-145 waits for the send/output branch after a successful receive EOF, bounded by the service idle deadline; a receive error retains the failure/cleanup path. Source inspection covered that changed path and the Unix input-drain/EOF mechanism, not a new physical-platform validation.



Historical status: open. Delegated scope decisions are accepted and the amended contract has been inspected; implementation correctness and release evidence remain unverified. No implementation, security, platform, latency, or release pass is claimed.

### Ownership and evidence baseline

This reviewer owns `docs/review.md`. Native client implementation is in progress; no native-client completion is claimed. Other agents own implementation and design amendments. Reference projects remain read-only. Design and secure-link/TMP actor checkpoints are recorded below; this ledger does not approve uninspected changes or unexecuted checks.

Baseline: `docs/design.md`, draft dated 2026-11-07, and `CAPABILITIES.md`. Neither document contains the implementation lead's latest delegated design decision at this snapshot. The workspace has no Git metadata at this checkpoint, so content hashes identify the reviewed documents:

- `docs/design.md`: SHA-256 `d657b03267105a8e3c9f551e15798777d03d5452375166392bb5e4a29002f8c5`.
- `CAPABILITIES.md`: SHA-256 `e776d0061a93c1e92178554b3e21215123d4ac971b5ac8c26de93942c36e946f`.

Sources read: TMP `docs/protocol-v1.md`, `docs/sdk-contract.md`, Rust release 0.1.0 notes and Rust/Node manifests; LightOS Remote Desktop, uu-remote-linux and RustDesk manifests, READMEs and license files; LightOS cross-platform host notes. No implementation test or Cloudflare account deployment was performed in this review. Nowledge CLI thread search `lightremote PSK review` returned no matches.

### Accepted v0.1 scope

These are implementation lead decisions under delegated design authority, not explicit user requirements or user quotations. Acceptance of scope does not mark implementation checks as passing.

- Attended sessions use a fresh, high-entropy 32-byte invitation PSK and one-use room tickets. The intended authenticated principal is the invitation holder. Persistent device identity, durable pairing and forward secrecy are outside v0.1; documentation must not claim them.
- The browser client must be delivered from an independent trusted origin. Relay blindness assumes an authentic client and uncompromised endpoints. A relay able to replace client JavaScript can obtain the fragment secret and plaintext. Invitation disclosure and endpoint compromise are outside that guarantee. Traffic timing, sizes and routing metadata remain visible to the relay.
- JPEG desktop streaming is experimental on one reliable ordered TMP link, with bounded queues and fair scheduling. No interactive latency guarantee is made. Cross-stream carrier blocking remains an architectural limitation. Three physical links are not required for this functional release and would require a separate change to the one-use room contract.
- The browser may alias the released Node SDK's session/framing modules into its build without forking the state machine. The implementation lead proposes this path; this reviewer has not yet inspected the alias or browser test evidence. WASM is not mandated.
- Relay agents will implement bounded, failure-closed Rust/DO behavior and tests. An OS capability/evidence matrix will distinguish compilation, automated functionality, physical desktop checks and signing.

The initial review's requirements for durable identity/PFS and three physical links are withdrawn as v0.1 blockers under this scope. Mature Noise library availability is not a prerequisite for reviewing the selected PSK construction.

### Scoped cryptographic assessment

No concrete cryptographic break has been demonstrated in the draft's high-entropy PSK + HKDF-SHA256 + directional AES-256-GCM construction. This is a construction-level assessment only, not an implementation approval or proof of security.

The fixed-width `client_nonce || host_nonce`, versioned room/role context, separate directional keys and exact counters provide a plausible binding for the stated scope. The implementation must enforce that contract. A canonical JSON transcript hash is not independently required if every accepted security-relevant field is strictly validated and already bound by the derivation/AAD. Unknown fields must not introduce unbound security semantics. Lack of persistent identity or PFS is a documented scope limit, not a newly discovered cipher flaw.

Before approval, verify:

1. The invitation PSK is the HKDF input keying material, never a public salt or relay ticket. All secrets and 32-byte nonces use a cryptographically secure RNG. Each endpoint contributes a fresh nonce for every session, including after crashes or restarts.
2. Version/type and opposite role are checked exactly; the nonce decodes to exactly 32 bytes. Room IDs have an explicit bounded grammar; UTF-8 derivation/AAD bytes, role ordering, HKDF salt/info and output length are identical across implementations. Handshake size, count and timeout are finite.
3. Initiator/responder and host/client mappings are explicit. Successful authenticated TMP negotiation is required before service authorization. A relay ticket alone never grants application permissions.
4. Per-direction nonces are unique under each key. Counters are unsigned 64-bit values, represented without JavaScript number precision loss; encoding is big endian. The receive counter advances only on successful authentication; duplicate, skipped or reordered records fail closed. Overflow and finite per-key record/byte limits close the session before reuse; the chosen limits and rationale must be documented.
5. Authenticate/decrypt before passing plaintext to framing or application code. Invalid tags, truncated records, duplicate hellos, write failure and timeout destroy the session. No retry may reuse a nonce with different plaintext. Cancellation must preserve exact write progress or close the carrier.
6. The 150000-byte outer limit is enforced before large allocations and includes all record overhead. The implementation specifies whether one encrypted record carries exactly one framed TMP packet or bounded chunks of the byte stream; both peers and relay tests use that same rule.
7. Secrets, invitation fragments, ticket-bearing WebSocket headers and application content are excluded from logs and diagnostics. Clear the invitation fragment after import; persist the PSK only if explicitly required by the attended-session flow.

Required evidence: fixed derivation/encryption vectors across Rust and the actual browser runtime; wrong-PSK, nonce/room/role tamper, reflection, bad tag, replay, gap, overflow and handshake-size rejection; fresh-session key separation; no service event delivered before authentication. Boundary tests may inject counters near their limits instead of generating enormous traffic.

### Findings and closure conditions

Priorities describe the original draft, not confirmed exploits. Implementation lead acknowledgment is not closure.

| ID | Priority / state | Finding or check | Required amendment and evidence |
| --- | --- | --- | --- |
| R1 | P0 / open, correction accepted | `design.md` says FIN, RESET and GOAWAY cancel workers. This conflicts with TMP half-close and drain behavior. | FIN closes one direction; RESET cancels that stream; GOAWAY rejects pending/new streams while accepted streams drain. Transport failure ends all streams. Tests cover upload FIN followed by commit/receipt, request FIN followed by download, output after input half-close, unread credit and GOAWAY drain. |
| R2 | P1 / needs validation | The crypto construction has no demonstrated break, but implementation details are not reviewed. | Add the explicit attended-PSK threat model and satisfy the cryptographic checks above. Do not report a security pass from roundtrip tests alone. |
| R3 | P1 / open, correction accepted | Bounds and independent I/O progress are asserted without numeric budgets or a precise actor lifecycle. Large-record assembly can deadlock if credit waits for an entire record. | Define per-stream and session byte budgets, queue item caps, pending OPEN limits, record assemblers, scheduler work budgets and overload action. One Session owner preserves returned packet order. Reader/writer progress continues under backpressure. Tests use tiny windows, saturated bidirectional queues and a record larger than its stream window; assert bounded memory and eventual progress or explicit closure. |
| R4 | P1 / needs validation, solution accepted | The released Node root export loads a Node-only transport module. | Inspect aliases into the pinned released distribution; show the browser bundle has no reachable Node-only runtime imports and runs real TMP handshake, fragmentation, WINDOW, RESET and close paths. Passing a Node test or merely bundling is insufficient. No state-machine fork is required. |
| R5 | P1 / open, implementation delegated | Rust/DO parity, waiting buffers, slow-peer handling and spent-ticket behavior lack executable evidence. | One black-box contract suite covers duplicate roles, expiration, waiting timeout, oversize traffic, overflow, disconnect and restart/DO re-creation. Lost state invalidates the room rather than permitting replacement. Account deployment is separate evidence. Reconnection needs a documented new-room/invitation flow; do not give the browser the operator token. |
| R6 | Accepted limitation / implementation checks open | One ordered link retains carrier head-of-line blocking and has no latency guarantee. | State this explicitly. Bound the JPEG queue; replace only complete frames that have not started transmission. Fairly schedule eligible TMP DATA with bounded control work. Never drop partially transmitted records or PTY/event bytes. Test functional progress during simultaneous video and file traffic; do not relabel that test as a latency guarantee. |
| R7 | P1 / open, matrix accepted | The capability map lists module responsibilities but does not show OS backend evidence or release status. | Add OS/architecture, feature, backend, build, automated test, physical validation and signing columns. Capability errors must be explicit. Windows/macOS CI success does not establish physical capture/input, permission, service or lock-screen behavior. Headless builds must prove real PTY/files without desktop dependencies. |
| R8 | P1 / needs validation | File commit, teardown, permissions and event completion are documented but not reviewed. | Verify capability-safe traversal and symlink/rename races, exact upload length, atomic no-overwrite commit, partial cleanup, shell resize/exit, child-tree cleanup and held-input release. Distinguish input EOF from session teardown. Define behavior when control notifications fail; do not silently retry input or destructive operations. |

### Module boundary follow-up

The existing crate layout need not be changed solely to match names from the first review. Keep logical ownership clear: encryption produces authenticated bytes; the TMP driver alone owns Session state and scheduling; application services own authorization and business framing; relays know only routing, tickets and opaque carrier records. The relay must not acquire an application or plaintext dependency. Separate OS backend implementations behind capability interfaces. These boundaries can live in modules until separate crates are justified.

### Evidence required for a pass

- **Amended design:** every accepted scope restriction is explicit; R1 is corrected; numeric budgets, record/counter contract and failure behavior are concrete. Record updated document hashes and review result.
- **Implementation core:** inspect the exact secure-link/TMP actor sources and associated tests. Record source hash or commit, commands, results and any unreviewed local changes. Cross-runtime vectors, half-close/drain, partial I/O and backpressure tests must pass before core approval.
- **Services and clients:** real headless PTY/files and browser sessions work; test fixtures are labeled. Inspect the NSH bridge and native build evidence separately. Synthetic images do not count as physical capture.
- **Relays:** the same contract tests pass for Rust and the local CF runtime. Typechecking alone does not prove DO pairing, resource bounds or restart behavior. Live CF deployment remains separately reported.
- **Release:** each advertised OS/architecture has its own evidence; physical desktop and signing gaps remain visible. Artifact names, checksums and license notices are checked. Unverified platforms stay experimental or unavailable, as appropriate.

Historical result: pending amended design and implementation evidence. No item above is closed merely because the implementation lead has accepted the proposed correction. No pass claim is authorized by this ledger.


### Core implementation checkpoint: source review, not a pass

Scope: the implementation lead requested `crates/core/src/{crypto,invite,link,records}.rs` and `crates/core/tests/roundtrip.rs`. This is an internal application component, not a published general-purpose library API review. Workspace edition is 2024; workspace MSRV is 1.88 (the core manifest does not itself inherit `rust-version`). Review concentrates on behavior rather than style or speculative API changes.

Source changed during this review. The findings below refer to this in-memory snapshot, not an assumed stable working tree. Line numbers identify this snapshot. Full SHA-256 values:

| Reviewed file | SHA-256 |
| --- | --- |
| `crates/core/src/crypto.rs` | `029e4eef687801084214bb1703d32ce5cbfe92fe91c63f8acc57419be747b3ef` |
| `crates/core/src/invite.rs` | `9c464673bb77f77581f5e701a81beae7053d30515fc04e3e28821cbd3f11b168` |
| `crates/core/src/link.rs` | `d3c652c8f321063856cf0962c9c1e41daae34aff98e29daac3d4525e239a3038` |
| `crates/core/src/records.rs` | `a58f5f528ba23372440dd0af1160436a90c9669a3ea951ac87b28d951f17a1f7` |
| `crates/core/tests/roundtrip.rs` | `a009a2f4c7553063bcd56bb220331c88b4fdfd807acc98a7c896ed749875b8d7` |
| `crates/core/Cargo.toml` | `f120607ce0cd637718b7637d2db52ff59e2b5beac2e314562d0c3f02580ed2f5` |
| `Cargo.toml` | `27143debf6bee7ebcc01a509a866c8b3e6e96565c91f9586fc1ead75d8f13748` |

No target-controlled build, test, formatter or linter was executed by this reviewer; implementation review was read-only and the implementation lead is running tests separately. The lead reported compilation and tests in progress; this is not independent pass evidence. No reference repository was modified. The only edited file is this ledger. A later checkpoint must recheck changed source and attach actual test output.

#### Must fix before core approval

**C1. P1, open: GOAWAY drain may discard its final queued packets.** Locations: `link.rs:135-136, 218, 263-269, 309-314`. Rule: TMP protocol v1 section 6 requires accepted streams to drain; section 1 forbids losing serialized write progress.

A last local FIN is considered sent when `emit` enqueues it. Once both FIN flags and accounting permit retirement, the stream is removed. If GOAWAY is active, the actor immediately exits when the stream map is empty. Dropping its I/O JoinSet aborts the writer even when DATA/FIN/WINDOW remain in the output queue or a send is in progress. A slow writer therefore allows an accepted response to be cut short during nominal graceful drain.

Smallest effective fix: add a writer drain/flush barrier after stream drain, with an explicit deadline and error result; do not abort the writer until all prior ordered packets have completed their writes. Local retirement alone is not proof that carrier output drained. Reproduction/closure test: deliberately park the writer, receive GOAWAY with an accepted stream, enqueue its final response/FIN, then release the writer. The peer must receive all bytes and clean FIN before the connection reports graceful completion. The current roundtrip test does not send GOAWAY.

**C2. P1, open: scheduler restart at the lowest ID permits starvation.** Locations: `link.rs:187-188, 211-223`. Rule: TMP protocol v1 section 6 and the delegated single-link scope require fair scheduling; assigning a class is not itself scheduling.

Each pump starts from the beginning of a BTreeMap. If two streams are pending and each connection WINDOW replenishes one chunk, a refilled lower-ID stream can consume that chunk on every pass, leaving a higher-ID stream blocked indefinitely. The assigned interactive/media/bulk classes are not consulted by the pump. This can starve a later shell/input stream behind an earlier bulk stream; it is not merely the acknowledged carrier head-of-line limitation.

Smallest effective fix: retain a rotating cursor or runnable queue across pumps, and use documented class weights if class priority is promised. Closure test: keep a lower-ID bulk stream pending while repeatedly restoring one chunk of connection credit; a higher-ID interactive stream must progress within a bounded number of scheduling turns. Reverse creation order as a sibling case.

**C3. P1, open: OPEN timeout/cancellation leaves pending streams allocated.** Locations: `link.rs:48-52, 117-120, 180-192`. Rule: TMP protocol v1 section 4 requires a host OPEN deadline and RESET; concurrent-stream limits include pending streams.

The timeout drops the oneshot receiver but sends no cancel command. Neither the actor nor a timer checks closed opening replies or expires pending OPEN state. A peer that never sends ACK/RESET can retain the stream, duplex buffers and workers after the caller has timed out. Repeating this fills the 32-stream limit and prevents further opens on an otherwise usable link. Cancelling the open future has the same issue.

Smallest effective fix: actor-owned pending-OPEN deadlines plus caller-cancellation detection, including queued opens whose reply is already closed. Reset and remove abandoned state even if the peer never sends another packet. Closure test: use paused time, withhold ACKs, cancel some opens and time out others; assert peer RESET, worker cleanup and capacity recovery without waiting for another inbound packet.

**C4. P1, open: invitation builder permits an unauthenticated remote client origin.** Location: `invite.rs:61-65`. Rule: the accepted PSK threat model requires authentic client delivery; relay URLs already enforce TLS outside loopback.

`Invite::url` permits any HTTP URL. A remote HTTP page can be replaced by a network attacker, and replacement JavaScript can read the invitation fragment, PSK and plaintext. Fragments being omitted from HTTP requests does not protect against hostile page content. This is a delivery trust-boundary flaw, not a demonstrated AES/HKDF attack.

Smallest effective fix: require HTTPS for non-loopback client URLs, retaining an explicit loopback development exception; reject URL credentials as for relay URLs. Closure test: external HTTP is rejected, HTTPS is accepted, and loopback HTTP development URLs still work. Independent origin trust remains a deployment responsibility; HTTPS alone does not prove the selected site is trustworthy.

#### Should fix and evidence still needed

**C5. P2, open: idle-failure and drain deadlines are absent from the actor.** Locations: `link.rs:173-174, 295-309`. Rule: TMP protocol v1 section 6 specifies host heartbeat/dead-peer handling and a drain timeout. The actor has connect/write/handshake timeouts but no ongoing timer, ignores Pong, and can remain alive indefinitely with a silent carrier or blocked accepted stream after GOAWAY. Add a timer and bounded liveness/drain policy. Test with paused time and a carrier that stays open but stops making progress; do not rely on receiving another packet to enforce the deadline.

**C6. Previous EOF/failure finding: mitigation observed, not closed.** Locations: `link.rs:18-27, 55-66, 135-136, 167-171`; `tests/roundtrip.rs:20-32`. The reviewed snapshot now has `MuxStream::completed()` and a failed status on state drop, plus a service-completion gate before successful FIN. These are substantive improvements to the earlier raw-Duplex EOF ambiguity. Callers still need to check terminal completion before committing a download as successful. A spot check saw CLI downloads adding that call; the native client is in progress and is not approved by this review. Add core tests that distinguish successful FIN from peer RESET, service failure, transport EOF and actor abort, including failure after a prefix of data. The current roundtrip test checks one successful receipt and uses a 100ms sleep; it neither calls `completed()` nor proves failure propagation. Replace timing assumptions with observable completion/barriers.

**Record cancellation contract.** `records.rs:6-19` correctly bounds lengths before allocation and reads incrementally through the duplex bridge, so a record larger than the TMP window need not wait for its entire body before returning credit. The helper is not cancellation-safe: cancelling after consuming part of a prefix/body or writing a prefix can desynchronize the same stream. Document that callers must preserve the future/progress or reset/close the stream; verify timeout/select users follow that rule. The native control loop spot check retains a pinned read across outgoing requests, which is the appropriate pattern. No concrete broken record caller was established in the requested core files.

#### Crypto checkpoint and retained mechanisms

In `crypto.rs:33-38, 48-74`, handshake parsing checks opposite role/version and exactly 32-byte decoded nonces. HKDF uses the invitation secret as input keying material; room/version/sender bind both derivation and AAD. Directional tags and counters form nonces, replay/gap checks precede decryption, and the counter advances only after authentication. In `link.rs:266-267, 278-289`, write/decryption errors leave the I/O worker and terminate the connection rather than transparently retrying. No concrete AES-GCM/HKDF break was found in these paths.

Keep the single Session owner, independent I/O tasks, bounded duplex/packet queues, `receive.outbound` ordering and separate FIN flags. Those are useful foundations. Verify their edge cases rather than replacing the state machine.

Remaining crypto evidence: cross-runtime fixed vectors, explicit per-key record/byte limits beyond u64 overflow, counter-boundary tests, nonce/session separation, handshake negative tests and confirmation that every outer failure closes the live session. The crypto unit test deliberately reuses a bare Cipher after a bad tag; this does not alone establish a live-session flaw because the connection I/O worker exits on that error. No security pass is claimed.

Checkpoint result: C1-C4 remain source-grounded must-fix findings in the recorded snapshot; C5 and C6 require the stated follow-up. R1/R3/R6 and the overall pass gate remain open until amended contracts, fixes and independent execution evidence are reconciled.


### Checkpoint 2: amended contract and formatted core recheck

Historical status: findings remain open. This checkpoint supersedes earlier statements that the amended contract had not been read. The implementation lead reports core compilation, three core tests and CLI smoke success. This reviewer inspected the current source and smoke script but did not independently execute them.

The amended `docs/design.md` now states delegated scope attribution, authentic-client delivery, PSK limits, single-link JPEG limits, numeric runtime/service/relay budgets and directional FIN behavior. `CAPABILITIES.md` now separates backend, build, automated, physical and signing evidence. Those requested documentation changes are present; their presence does not establish runtime conformance.

| Earlier item | Latest state |
| --- | --- |
| R1 lifecycle contract | Document correction inspected; runtime C1, C7 and C8 remain open. |
| R3 budgets/credit | Numeric limits inspected; execution and cancellation checks remain open. |
| R7 OS evidence matrix | Matrix inspected; pending CI/physical/native evidence is still pending. |
| C1 GOAWAY output drain | Still present in this snapshot, `link.rs:280-286, 434-445, 612-617`. |
| C2 scheduler starvation | Still present, `link.rs:434-455`: sorted ID iteration restarts on every pump. |
| C3 cancelled/timed-out OPEN | Still present, `link.rs:106-119`: no actor cancellation/deadline cleanup. |
| C4 remote HTTP invitation URL | Still present, `invite.rs:89-96`. |
| C5 heartbeat/drain deadline | Still absent from the actor select loop, `link.rs:598-612`. |
| C6 failure status | Controller completion-status mitigation retained; host-side failure/FIN distinction remains broken as C7 below. |

#### C7. P1, new: RESET can be mistaken for request FIN before upload commit

Source locations: `crates/core/src/link.rs:137-145, 352-360`; `crates/cli/src/main.rs:160-162`; `crates/services/src/records.rs:49-54`; `crates/services/src/files.rs:194-205`.

Rule: the amended application contract requires exact-size validation plus actual peer FIN before upload commit. A RESET cancels the stream; transport EOF is not a stream FIN. Controller-side terminal checks do not establish the host's received-half-close condition.

The actor handles peer RESET by deleting LocalStream and aborting the bridge tasks. The remaining application DuplexStream sees ordinary EOF. The host dispatcher passes only `incoming.stream.io` to the service; it does not pass a peer-FIN/failure signal or cancel the service worker on a per-stream RESET. `expect_fin` accepts any EOF, then `Files::put` hard-links the staging file to the destination. A later failed receipt does not undo that committed destination.

Minimal source-derived counterexample: OPEN a zero-byte `file.put`, wait for acceptance, then send RESET without DATA/FIN while keeping the connection alive. The service can interpret the dropped bridge as FIN and create the destination after cancellation. The same issue applies after the exact declared payload has arrived. Zero bytes avoids depending on data-consumption timing. This path is not covered by a successful upload or a nonexistent-download test.

Smallest effective fix: deliver an error-aware application read boundary, or separately expose authenticated peer-FIN/failure state to the host service and check it before commit. Also wire RESET into service cancellation. Do not wait for the existing full `completed()` before committing: it requires local FIN too, and local FIN is gated on service completion/receipt, so that would introduce a cycle. A received-half-close check must complete independently of outgoing completion.

Closure tests: zero-byte and nonempty exact-size uploads followed by RESET without FIN leave no destination and clean their staging files; unrelated streams stay usable. A valid FIN followed by commit still emits a verified receipt. Include reset after the size bytes but before the service's EOF check, and define the commit race boundary explicitly. No dynamic reproduction was run in this review.

#### C8. P1, new integration finding: shell request half-close stops output immediately

Source locations: `crates/services/src/shell.rs:100-131`; related smoke assertion `scripts/e2e-cli.py:31`.

Rule: the amended lifecycle contract closes only the sending direction on FIN. A shell input half-close is not by itself a whole-session teardown.

The shell receive future returns success on input EOF. The outer select then ends the service and calls `session.stop()`, killing the child and abandoning output even when commands or output are still pending. Thus the core's directional FIN flags alone do not ensure end-to-end shell half-close behavior.

Smallest effective fix: after a genuine input FIN, stop the input pump while retaining the output/child-exit path under a bounded deadline. RESET/session loss should take the cancellation path and terminate the child. Distinguish those events using the host-side state from C7.

The current smoke sends a literal `LIGHTREMOTE_PTY_OK` string inside a command and checks only whether that substring appears in output. PTY echo can satisfy that assertion without proving printf executed or output drained. This does demonstrate a real PTY path, but cannot close the shell execution/half-close gate. Use an expected marker absent from the input bytes (for example output encoded using escapes), disable echo before the assertion where possible, and test delayed output after the client sending half is shut down. Assert the output and child exit status. A RESET sibling must terminate/reap the child instead.

#### Credit and cancellation assessment

No definite double-consume/double-refund bug was found in the inspected actor path. Bytes are consumed only after a bounded duplex write. A queued Consume for a stream already deleted by RESET is ignored, while TMP returns outstanding connection credit during RESET removal; this avoids applying the same refund twice. WINDOW packets with no events still reach `pump()`. Outbound writes remain in a separate task, and a failed queue admission closes the link rather than rolling Session state back.

This is source evidence only. Required stress cases still include RESET with a queued Consume, buffered inbound bytes, a pending outbound chunk and late DATA; unrelated streams must continue and connection credit must not inflate or remain permanently withheld. Tiny windows and saturation remain untested by the visible roundtrip test.

The three visible core tests are crypto direction/room/order checks, bounded-record rejection and one successful encrypted 1MiB half-close exchange. The roundtrip still uses a 100ms sleep and does not independently exercise GOAWAY, reset/cancellation, terminal failure status or tiny windows. The CLI smoke script creates a fresh attended host/room per operation; its happy paths do not establish concurrent-stream fairness or post-cancellation isolation. Shared WebCrypto/Rust vector work is reported in progress, not verified here.

Reviewed snapshot hashes (integration files were read only to follow the core's failure/FIN consequences):

| File | SHA-256 |
| --- | --- |
| `crates/core/src/crypto.rs` | `1c4d0fb1fd2f713b01346fc1706438775caf7602bb02ad3c76870d13da79d8c0` |
| `crates/core/src/invite.rs` | `530b385e57b769455f35c545b5448329e06fdd922bd8999cf6337f7e05a2c2eb` |
| `crates/core/src/link.rs` | `ff3590f81c1cba1bc3e8976baf8f5edfe8c8131f0455a69bf47ce95bc26f215c` |
| `crates/core/src/records.rs` | `a4be6c712cc6d76fef3435fe299f8fb1c018920185cb576ec53022332ce1f323` |
| `crates/core/tests/roundtrip.rs` | `0a36e20aec1a75b54814dff2c5168e66373d2c85e289b75b719d1c4ce0101070` |
| `crates/services/src/files.rs` | `f9de7c79a544bb394ddda3ff0293e1818d76db6418ce7b7ebbda6651c66d2a0c` |
| `crates/services/src/records.rs` | `1e0d6844027f797ec72d1328de6944d895f1aceea9ffc73d8ae5e9b95b6716fd` |
| `crates/services/src/shell.rs` | `864e769bc5abb047c7243fde289088748d08e2152ee3126e9329c55bb7a78fd6` |
| `crates/cli/src/main.rs` | `d8137fc0855543ef37771da4ecf5d108497e44a61421c5a9580fb99f04fc8449` |
| `scripts/e2e-cli.py` | `7a5d1c9c8ac09c27bebc71734dff74740b3a9cc80f3c84c0ca570f5e1be90a8c` |
| `docs/design.md` | `e151028d1052397985a8e7f79487073ccebaec995c4dc9a6acc282136463368b` |
| `CAPABILITIES.md` | `e8407961de88747f1c3689fc041e366d5091b01570162f19e912570bece09e32` |

No code changes were made by this reviewer. Do not promote this checkpoint to PASS from the reported happy-path smoke results. Recheck fixes against exact source hashes and independent regression output.

Post-check: these files changed after checkpoint 2 and need a further review; no finding is closed by this comparison:
- `crates/services/src/shell.rs`: current SHA-256 `9425aebf18ef877f02019260deaff42657fa9c12d099e1772cf4234b91fd095b`; reviewed hash is in the checkpoint table.

Follow-up spot read of the changed shell source: the child-wait closure now retains a semaphore permit; the receive-EOF/select/session.stop path at lines 100-124 remains unchanged, so C8 remains open. Other concurrent service-test edits were not reviewed or executed here. Git metadata now exists with no commits on main; git status for the only reviewer-edited file is `AM docs/review.md`.


### Checkpoint 3: publish-facing status, defects versus missing evidence

Current result for the recorded source snapshot: **no P0 found; five source-grounded P1 findings remain open**. These are control-flow findings with concrete triggering scenarios, not claims that a new runtime reproduction was executed. Reported core/CLI test success is recorded as implementation-lead evidence; this reviewer did not rerun those commands.

| Item | Classification now | Source status / required action |
| --- | --- | --- |
| C7 RESET before upload FIN | P1 source defect, open | Core RESET still drops the bridge; CLI host still passes raw DuplexStream; expect_fin still accepts any EOF before hard-link commit. Add authenticated received-FIN/error state and service cancellation. Zero-byte OPEN then RESET without FIN is the minimal regression. |
| C8 shell half-close | P1 source defect, open | Shell receive EOF still wins select and leads to session.stop. Preserve output/child-exit processing after normal input FIN; cancel on RESET. |
| C2 scheduling | P1 source defect, open | pump still rebuilds sorted IDs and restarts from the beginning, without a retained cursor. Repeated one-chunk connection-credit replenishment can favor a continuously refilled lower ID. This is a fairness failure, separate from any latency promise. |
| C3 abandoned OPEN | P1 source defect, open | open still only times out its reply receiver. Actor-owned cleanup for expired/cancelled pending opens is absent; a no-ACK peer can occupy the stream limit after callers abandon the opens. |
| C4 remote HTTP client URL | P1 conditional delivery defect, open | Invite::url still allows external HTTP. This triggers only when a remote HTTP client URL is configured; the default loopback development URL does not itself trigger this finding. Enforce trusted HTTPS delivery outside loopback. |
| C1 GOAWAY queued-write loss | Source mitigation observed; drain regression pending | New ordered Output::Finish waits behind queued packets, closes the writer and acknowledges under an actor-side 20-second deadline. The previous immediate-writer-abort finding is no longer counted as an unfixed P1 in this snapshot. Check slow/full queues, error propagation and final DATA/FIN delivery. |
| C6 controller reset/EOF status | Source mitigation observed; boundary regressions pending | completed() provides terminal failure status. This is not itself a remaining missing-controller-status defect. C7 is the separate unresolved host-side received-FIN problem. |
| shared crypto vector | Test implementation inspected; execution reported, not independently rerun | Rust and web tests load testdata/secure-link-v1.json. Rust asserts the complete record; the web case also reconstructs it using Node hkdfSync/createCipheriv. No concrete AES-GCM/HKDF break was identified. |
| tiny-window/reset-credit tests | Evidence gap | Missing stress output is not evidence of an arithmetic bug. No definite double-refund/overconsume bug was found in the source paths inspected. |
| C5 liveness deadline | P2 source omission | Ongoing heartbeat and accepted-stream drain deadline are still absent; the new final writer-flush deadline does not bound the earlier stream-drain phase. |

The new writer error forwarding preserves ordering through the actor input queue, instead of letting a completed I/O task automatically overtake queued decoded packets. CarrierClosed handling was inspected as a concurrent change; it is not a substitute for reset/credit/drain regressions.

**C9, P2 source edge case:** in the new reader, Message::Close returns CarrierClosed before decoder.finish(), while socket EOF checks finish(). If encrypted plaintext has left a partial TMP frame after all streams have FIN flags (or with no streams), a WebSocket Close can be classified as graceful rather than a truncated-frame protocol error. Always check decoder.finish() for both close paths before selecting graceful-carrier handling. Regression: completed stream plus a partial following TMP frame and WS Close must fail. This is a protocol-validation defect, not a new P0/P1 cipher finding.

Review source snapshot, including the concurrently added writer barrier and shared-vector tests:

| File | SHA-256 |
| --- | --- |
| `crates/core/src/link.rs` | `6da494968e18a1b9abf4c3deb97b4c2ab2c3098b8fdfc320f54470a44de9f5d4` |
| `crates/core/src/crypto.rs` | `1aca06dfe2e53033fa780c8c6706935f346c1fe96ab0520ed3e2d860871dc1ab` |
| `crates/core/src/invite.rs` | `530b385e57b769455f35c545b5448329e06fdd922bd8999cf6337f7e05a2c2eb` |
| `crates/core/tests/roundtrip.rs` | `0a36e20aec1a75b54814dff2c5168e66373d2c85e289b75b719d1c4ce0101070` |
| `crates/services/src/files.rs` | `f9de7c79a544bb394ddda3ff0293e1818d76db6418ce7b7ebbda6651c66d2a0c` |
| `crates/services/src/records.rs` | `1e0d6844027f797ec72d1328de6944d895f1aceea9ffc73d8ae5e9b95b6716fd` |
| `crates/services/src/shell.rs` | `786ad4236788bbae0ebe9057be39040a138f8fc5bad88b33dca9437d96605af2` |
| `crates/cli/src/main.rs` | `d8137fc0855543ef37771da4ecf5d108497e44a61421c5a9580fb99f04fc8449` |
| `testdata/secure-link-v1.json` | `80973609edb3ab1ea3e5e725b6245ac065541d1d97fd7919bfa5614253042c51` |
| `web/tests/secure.test.ts` | `877df24177d95794ef40707df740963f8f1d6fe483da70bd5e09736ff61907ac` |

Release implication: current refusal to approve is due to the five P1 source paths above. Test additions alone do not resolve those paths. Separately record which fixes have been inspected and which regression commands actually passed; do not describe every missing test as a confirmed defect or imply a scope requirement for persistent identity/PFS or multiple physical links.


### Checkpoint 4: exact drain/FIFO code and relay closure defect

This is a review of current implementation changes, not a repetition of the original draft findings. Source-only result: the earlier core C1 immediate-writer-abort path has been replaced. No execution result is claimed for the newly running test.

Exact reviewed core locations (`crates/core/src/link.rs`, SHA-256 `edaf107fd52d8678584e9e6d64591444bef3873e39babee4f6053f67346881c3`):

- `LinkHandle::drain`: 107-113; actor Command::Drain invokes Session::goaway: 459-464.
- Ordered reader delivery: decoded packets enter the input queue at 633-634, and terminal errors enter the same queue at 646-647. Actor join_next at 672 only reaps/checks worker panic; it no longer turns ordinary worker completion into an error that can overtake queued packets.
- Graceful remote Close: 665-667 requires every remaining stream's local and remote FIN flags before retaining the stream workers to drain buffered application bytes. Later Consume processing and retire/accounting at 306-314 and 450-455 still complete the streams. emit at 180-188 suppresses credit writes after this terminal carrier close.
- Writer Finish: 600-603 closes only after preceding FIFO output packets. Actor enqueues Finish and waits with a deadline at 683-691. This addresses C1's original lost-final-queue path; failure and overload regressions remain separate.
- Private connect_inner accepts settings at 501-506. New test at 714-782 uses DATA=1024, stream_window=4096, connection_window=65536; sends GOAWAY after an accepted stream, transfers 2MiB in each direction, checks completed(), exact response bytes, and host task completion. This is substantive test coverage in source; its running/passing output was not independently observed here. It does not test multi-stream scheduling, abandoned OPEN or reset-before-upload-FIN.

**C10. P1, new source defect: relay cancellation can overtake queued forwarded records on clean Close.**

Current relay locations: `crates/relay/src/lib.rs:141-143, 386-410`; reviewed SHA-256 `3238cd8f8cae478bf4b6f8ea79c140655bc35d4116f8b9fa1d9cc558d3ce2e8d`.

At 396, an authenticated endpoint's Binary record is only queued into the other endpoint's mpsc receiver. A following WebSocket Close takes the catch-all break at 400. The cleanup at 409 removes the room and sends its cancellation notification through consume at 143. In the peer's loop, the top check at 387 and biased select at 391-392 favor cancellation over queued messages at 403-405. Therefore the last accepted DATA/FIN can be discarded before being forwarded, even though the core writer successfully flushed it to the relay before Close.

Concrete schedule: queue the final Binary/FIN to the destination, let the source loop read the following Close before the destination loop writes its queued record, then let the destination poll cancellation. The destination closes without receiving FIN. The new core actor FIFO cannot repair bytes lost inside the relay. This can explain a drain-test truncation, but no test failure is attributed to it without observed output.

Smallest effective fix: distinguish normal endpoint Close from expiry/overflow/write failure. For normal Close, invalidate the room for new joins but preserve forwarding order: drain previously accepted records or enqueue a terminal marker behind them in the same destination FIFO, with a bounded write/drain deadline. Hard failure may abort explicitly. Do not broadcast an immediate cancellation before the valid queued prefix has been forwarded.

Closure regression: park the destination writer, forward one or more distinctive Binary records followed immediately by source Close, then release the writer. All queued records must arrive in order before destination Close. A second case exercises overflow/timeout failure. Re-run the core tiny-window/drain test after this relay fix as well.

Priority separation at this checkpoint: no P0 found; C2/C3 remain core P1 source defects, C7/C8 remain host-integration P1 source defects, C4 remains a conditional remote-client-delivery P1, and C10 is the newly identified relay P1. C1 is source-mitigated, not an unfixed P1. C9 (Close bypasses decoder.finish at 638) remains P2. Missing test executions are listed as evidence gaps, not added defects.


#### Checkpoint 4 follow-up: concurrent PONG/write-error changes

The implementation changed again during review. Re-read source SHA-256: `295d514ac59b8831aebc13cc54a146e411f2718ae828e53d0b13ed8a72471639` for `crates/core/src/link.rs`. This later checkpoint supersedes earlier current-line references where the additions shifted them.

**C11. P1, new core source defect: a real writer failure is reclassified as graceful completion from FIN flags alone.** At line 681, the benign-close guard now accepts both CarrierClosed and CarrierWriteFailed when all remaining LocalStreams have both FIN flags. At 491, local_fin is set when a packet is only enqueued, not when the writer completes it. At 316-323 the runtime may already remove that stream and mark Completed, making the all-streams predicate vacuously true. Thus the predicate cannot establish that queued local application DATA/FIN reached the carrier.

Concrete schedule: the peer has normally half-closed its request direction; the local service enqueues its response tail and FIN and retires its LocalStream; the writer then fails while sending that response tail/FIN. The error at 681 enters the graceful branch, the empty runtime exits at 693-694, and the Finish flush at 707 is skipped because carrier_closed is true. Connection.task can return Ok although an actual queued application write failed. This violates the failure contract independently of whether a happy-path drain test passes.

Smallest effective fix: do not suppress CarrierWriteFailed from the both-FIN predicate. Return it as terminal failure unless independent writer progress proves all required application output had already completed and the failure concerns only optional post-completion traffic. Local Session state and map retirement are not that proof. Regression: hold the writer, enqueue response DATA/FIN after remote FIN, then fail that write; Connection.task must fail. A separate case may allow redundant control-write failure only after confirmed application-output completion, if explicitly designed.

New drain_probe/PONG state at 404-405 and 693-698 is an end-to-end prefix barrier, which can mitigate C10's relay-close tail loss for this TMP protocol. Keep it distinct from flushing to the relay. It needs its own timeout while waiting for the PONG; the 20-second Finish timeout at 713-715 is reached only after that wait ends. Do not count this new prefix-ack wait as bounded merely because the later writer Close is bounded.

C10 classification refinement: relay queue cancellation remains a source-confirmed behavior, but the new TMP end-to-end PONG barrier addresses its normal-session trigger by confirming that the peer processed the ordered prefix before local Close. C10 is not an additional unconditional core P1 once that barrier and failure handling are sound. Decide and test the relay's own normal-close contract explicitly; if room disconnect intentionally aborts unforwarded records, document that behavior and require end-to-end completion before disconnect. Do not infer a universal relay-drain requirement from the core's local socket flush alone.

Core-only P1s still open in this late snapshot: C2 scheduler fairness, C3 abandoned OPEN, and new C11 writer-error suppression. C1 has a source fix; the FIFO reorder fix and new test are present. C7/C8 are separate host integration defects and C4 is the conditional client-delivery defect; their exact source paths were recorded earlier. C9's WebSocket Close truncation check remains a P2. No P0 found. No running test is represented as independently passed.


### Scoped re-review: C2, C3 and C11 only

**Source result: the three reported defects have source fixes; no remaining P0/P1 was found in this scoped re-review. Regression execution evidence is pending.** This result supersedes prior open-source-defect status for C2/C3/C11 at older hashes. It does not close unrelated ledger items or claim a whole-product release pass.

Reviewed file: `crates/core/src/link.rs`, SHA-256 `54945ee16b57fc3a7a8b85c09f92434e3466ec46d46ecc351d32b3a82ad04521` (983 lines).

| Item | Current source evidence | Scoped disposition |
| --- | --- | --- |
| C2 persistent low-ID preference | pump at 529-557 iterates IDs greater than cursor, then wraps through IDs at or below it (533-535). Cursor advances on successful Session.send at 543, while a blocked send preserves its pending chunk. Removal of the cursor stream does not break the BTreeMap ranges. | Original fixed-order starvation path resolved in source. Unit case at 873-894 keeps both streams pending, returns one 1024-byte connection-credit chunk, refills the first stream and checks that the second wins the next pass. This establishes the intended rotation case; it is not a claim of weighted byte fairness. |
| C3 pending OPEN after caller cancellation/timeout | cancel_unclaimed_opens at 513-527 finds closed opening reply senders and invokes reset. Maintenance ticks every second at 729/748-749, independently of network activity. LinkHandle's timeout drops its receiver, making that opening eligible for cleanup; runtime removal drops its task handles and Session.reset removes the protocol stream. | The historical absence finding is superseded by current maintenance/cleanup code. Unit case at 895-901 explicitly closes the receiver, runs cleanup and checks stream removal. There is no requirement to retain C3 as a source defect solely because the older checkpoint lacked this mechanism. |
| C11 empty stream map hiding queued/failed application writes | emit at 197-223 increments write_pending before publishing each critical packet; failed try_send rolls that increment back. Only WINDOW/PING/PONG are noncritical, so every DATA, including empty FIN, is counted. Writer at 677-685 decrements only after successful SinkExt.send; failures and timeouts return before decrementing. carrier_fully_written at 508-511 also requires ready and both FIN flags; the benign-close guard at 741 calls this predicate. | Original FIN-only suppression path resolved in source. Empty-map test at 905-913 confirms an emitted DATA keeps the predicate false after stream removal, and only a simulated successful writer decrement makes it true. The simulation is a predicate regression, not an executed faulty-sink integration case. |

SinkExt.send's flush semantics were checked in the installed futures-util source: its send future returns success only after poll_flush succeeds. Therefore the counter's successful decrement is after the sink flush, not merely after placing a record into the split sink. The pre-enqueue increment also prevents a fast writer from decrementing before the count exists. The counter stays positive if encoding, encryption, send or timeout fails before a successful critical write. No counter underflow/race was identified in the reviewed production paths.

Additional directly related lifecycle observation: maintenance wakes the actor during a silent drain; the 60-second deadline at 755-757 bounds the accepted-stream/PONG phase, including when no peer event arrives. The later Finish timeout remains a separate writer-close bound. This addresses the previously identified unbounded probe wait in source. This note does not claim a heartbeat audit.

Visible regression cases to match against execution output:

- `fair_credit_rotation_and_unclaimed_open_cleanup`
- `empty_stream_table_cannot_hide_unwritten_data`
- `tiny_windows_and_goaway_preserve_large_half_closed_transfer` (917 onward): 1KiB DATA, 4KiB stream window, 64KiB connection window, 512KiB each direction, terminal completion and host drain completion.

The current tiny-window fixture is 512KiB each direction; older checkpoint references to 2MiB describe older source. Preserve this distinction when reporting evidence.

For final scoped sign-off, attach a successful `cargo test -p lightremote-core` result (or the three named cases plus the relevant core suite), including exit status and the source hash tested. A source change to the reviewed paths requires a delta recheck. This reviewer did not execute target code in this read-only review. Do not keep these three labeled as confirmed unfixed P1s while waiting only for their test result.


</details>
