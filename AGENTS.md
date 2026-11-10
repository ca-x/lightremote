# roc-remote engineering rules

- TMP remains a transport-independent library from lib-x/tm. Put service-specific
  behavior in versioned roc-remote application profiles, never in TMP messages.
- Do not copy source from the reference projects: their licenses differ. Keep
  implementation original and record dependency provenance in THIRD_PARTY.md.
- Every permission defaults off. File operations remain inside the opened share
  directory capability. Upload success requires exact size, FIN and atomic
  no-overwrite commit; failed/cancelled uploads remove staging files.
- Never log invitation URLs/secrets, role/operator tickets, keystrokes, terminal,
  clipboard or file contents. Do not automatically replay side-effect requests.
- One task owns each TMP Session. Its reader and writer progress independently;
  queue/record sizes remain bounded and WINDOW always resumes sending.
- FIN is a half-close; RESET is stream failure; GOAWAY refuses new work and drains
  accepted streams. EOF after RESET/transport failure is not download success.
- Separate encrypted-carrier, TMP-runtime and host service responsibilities.
  Relays must depend on no plaintext or application service implementation.
- Headless builds exclude capture/input/clipboard/NSH dependencies. Platform
  capability failures are explicit, never synthetic successful captures.
- Run scripts/check.sh and relevant end-to-end scripts before claiming completion.
  Linux desktop builds require Clang/PipeWire; NSH requires GTK4/GStreamer.
  Record hosted build, real OS desktop, and signing evidence separately.
- Release tags are immutable. GitHub Actions produces per-platform/product
  archives and checksums. No live Cloudflare deployment is implied by local tests.
