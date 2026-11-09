# Native controller design

## Scope and intent

Desktop Rust NSH controller for Linux, macOS and Windows. Mobile, Web and TUI
are not targets of this binary; the browser client remains a separate product.
Keep the OS title bar and system font. A remote session, not transport internals,
is the organizing object. No host mode, stored invitations, device history,
automatic clipboard synchronization or request replay is introduced.

## Airhand reference, 2026-11-09

Reviewed upstream README, `docs/usage.md` and `docs/security.md` at
https://github.com/sqlpxc/airhand using GitHub's API. Upstream declares MIT.
Only product behavior was studied; all implementation here is original.

| Before | After | Why |
| --- | --- | --- |
| Every tool in one scrolling form | Stable connection header and Desktop / Terminal / Files / Clipboard / Session tabs | One task at a time without losing session context |
| Permission JSON mixed with display results | Human-readable permission rows, separate display results | Actions reflect the host's actual capabilities |
| Always-enabled remote operations | Disabled controls and nearby permission explanation | Distinguish disconnected, unavailable and denied states |
| Hard-coded light colors | App-owned semantic light/dark tokens and shared controls | Consistent contrast, focus and native appearance |
| Empty black preview | An explicit no-frame state and a separately armed input state | Never imply that a capture or control operation succeeded |

Other worthwhile future work from Airhand's product model: actual-size / fit
viewport controls, capability-negotiated codecs, adaptive video quality, and
latency telemetry based on measurements. These need separate application-profile
specs and platform validation; this change does not claim to implement them.
Do not adopt short control codes, relay-terminated plaintext, privileged host
defaults or automatic side-effect replay. Keep LightRemote's encrypted carrier,
TMP runtime and permission-gated host services separate.

## Foundations

- `crates/native/src/ui/theme.json` owns all literal colors, paired light/dark
  surfaces, primary / on-primary, muted text, borders, input and focus colors.
- `ui/controls.rs` owns button variants, fields, rows, panels and typography.
  Pages compose these controls. Native base controls own keyboard and focus.
- System font; 12–14 pt supporting copy, 16 pt content, 20 pt workspace titles.
  Spacing uses NSH's 4 pt scale: 8 within a row, 16 between controls, 20 at a
  panel boundary, 24 between task groups. Rounded controls use the theme radius.
- Use text-labeled tools instead of unlabeled icon guesses. Status always has a
  text label, never just a color. No fabricated latency, frame rate or security
  status. Never show or persist invitation secrets after submission.
- Connection and navigation remain outside each workspace's scroll owner.
  Toolbars wrap in narrower windows; fields shrink with `min_w_0`. Default
  window 1120×860, compact validation viewport 760×700; no mobile claim.
- Keep tab contents mounted so drafts and the terminal survive navigation.
  Workspace panels share one positioned frame and use `invisible`, not
  `display:none`: NSH Terminal otherwise resizes its local emulator and the
  remote PTY to 1×1 when its layout disappears. Inactive panels retain geometry
  but take no input or accessibility focus. A red/green regression covers this.
  Changing tools releases remote input; leaving a tool does not replay requests.
  Files remain relative to the opened share and commits never overwrite files.

## Motion and feedback

The frequency gate rejects animated tool switching, terminal input, pointer
forwarding and repeated shortcuts. Immediate native hover / focus / press
feedback is intentional, not missing animation. Tabs do not use NSH's optional
moving indicator. No artificial delay or gesture choreography is introduced.
This static motion policy also honors reduced motion without a second path.
No translucency is required for legibility, and no sound is added.

## Verification

Use native headless interaction tests for role/name navigation, disabled
operations, connection failure, theme token parsing, dark mode and reduced
motion. Run native check / tests / Clippy, `scripts/check.sh`, and CLI end-to-end
tests separately. Headless results do not establish the actual OS appearance,
screen-reader behavior, signing, hosted CI or Cloudflare deployment. Record
unavailable real-window evidence explicitly.
