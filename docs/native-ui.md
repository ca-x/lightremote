# Native controller design

## Scope and intent

Desktop Rust NSH controller for Linux, macOS and Windows, with a separate
experimental Android viewer entry. Web and TUI are not targets of this binary;
the browser client remains a separate product.
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
defaults or automatic side-effect replay. Keep roc-remote's encrypted carrier,
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
  window 1120×860, compact desktop validation viewport 760×700. Android has a
  separate single-scroll viewer; it does not expose the desktop tool workspaces.
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

## RustDesk and uu-remote-linux review, 2026-11-10

The primary task is operating one authenticated remote session. The Rust client
owns connection, capabilities and operation lifetime; NSH signals own only local
drafts, selected workspace and input arming. Shared semantic tokens and controls
remain authoritative. Reference behavior is adapted to NSH, not transplanted
from Flutter or GTK. No new UI, motion or platform-injection dependency is added.

Reviewed RustDesk's `flutter/lib/desktop/widgets/remote_toolbar.dart`,
`flutter/lib/desktop/pages/remote_page.dart`,
`flutter/lib/mobile/pages/remote_page.dart` and `src/platform/macos.rs` for
session-oriented controls, explicit focus ownership, mobile input boundaries and
separate screen/input consent. Reviewed uu-remote-linux's
`src/desktop_session.rs`, `src/video_select.rs` and `src/capture_setting.rs` for
backend selection, cancellation and the distinction between available and
requested settings. Only requirements and behavior were borrowed.

| Before | After | Why |
| --- | --- | --- |
| Invitation form takes space throughout a connected session | Form is hidden after authentication; Cancel/Disconnect stays in the header | Keep the active task and escape path visible without retaining the secret |
| Remote surface consumes keyboard navigation | Ctrl+Alt+Esc releases remote input and returns focus to the local window | Keyboard users need an explicit exit from remote focus |
| Repeated file/clipboard presses can start concurrent effects | Bounded per-operation single-flight state and nearby progress text | Prevent duplicate side effects without retries or simulated progress |
| Directory results can arrive out of order | One listing at a time, with the last successfully loaded path shown | A draft path must not relabel old results |
| Generic service-unavailable states | Session shows host-specific platform guidance independently of policy | OS consent, build features and host grants are different boundaries |
| XWayland can make an X11-only injector look ready | Wayland input is refused both in probing and real input construction | Never promise full-desktop control through an unsupported backend |
| Desktop controls below the touch minimum | Shared actions, fields, selects and tabs have 44-point minimum height | Preserve native interaction feedback while supporting touch targets |
| No Android app entry | Separate explicitly experimental, read-only Rust NSH viewer | Try genuine packaging without pretending desktop tools work on mobile |

The desktop native surface is proportional to window height; this is media
geometry, not an arbitrary spacing token. Inactive workspaces remain `invisible`
to preserve terminal dimensions. Keyboard/pointer forwarding and frequent tab
changes remain immediate; reduced-motion mode requires no separate animation
path. Standard headings and errors carry Header/Alert accessibility semantics.

Linux/macOS request focus on the local Desktop tab after release. The pinned
NSH Windows runtime cannot programmatically focus ordinary tabs and its Tab
traversal is limited to text fields. The Windows surface therefore intercepts
the chord in its own Win32 key handler before forwarding Escape, releases mouse
capture and remote held state, and focuses its parent NSH window. This is a
local-focus escape, not a claim of complete Windows toolbar keyboard traversal;
toolbar actions there still need pointer interaction. Windows runtime verification
remains outstanding.

Android uses a native NavPage inside NavStack, a single scroll owner, safe-area
insets and keyboard-height avoidance. It connects only on explicit submission,
starts display 0 capture only on request, and never sends input events. Clipboard,
terminal, document picking and local file transfers are deliberately absent;
adding these requires real platform implementation and device validation, not
desktop filesystem paths. See `docs/android.md`.

## Icons and contextual help

The primary task remains explicit remote access, not icon exploration. Existing
workspace state and connection/operation owners are unchanged. Shared controls
provide static Kubai SVGs, named workspace tabs and contextual help. Desktop
hover or keyboard focus shows NSH's Tooltip; pressing the same named, 48-point
help trigger expands inline text. Touch uses the inline disclosure, not hover
or long press. A second press collapses it. There is no ornamental motion or
automatic remote action. The web equivalent uses native details/summary,
pointer-only tooltip styling and Escape dismissal with focus retained.
Native Session and Android permissions also expose the full MIT icon notice
through an optional Icon licenses disclosure; long license text is not a tooltip.

| Before | After | Why |
| --- | --- | --- |
| Text-only native tabs and a different web icon style | Shared static Kubai icons plus short visible tab labels | Recognizable tools without sacrificing discoverability or accessible names |
| Persistent desktop input and terminal instructions | Short safety line plus Desktop / Terminal help | Keep escape and destructive-process behavior visible; defer secondary explanation |
| Help that would require a mouse to discover | Named help trigger expands on click, keyboard or touch | A mobile user can read the same information without a hover gesture |
| Repeated lengthy session explanation | Session help disclosure | Less reading during an established session, with details still available |
| Potential animated icon runtime | Six bundled static MIT SVGs | No new JS runtime, hover-only promises or distractions during remote input |

Permission denials, host guidance, transfer outcomes, invitation secrecy,
clipboard replacement and the experimental Android feature boundary remain
visible. Connection, transfer and stop actions retain labels. Icon-only help is
deliberately avoided on touch. Inactive terminal workspaces remain invisible,
not display:none. Platform keyboard-navigation limitations noted above still
apply; these changes do not establish Windows or Android device validation.

## Verification

Use native headless interaction tests for role/name navigation, disabled
operations, connection failure, theme token parsing, dark mode and reduced
motion. Run native check / tests / Clippy, `scripts/check.sh`, and CLI end-to-end
tests separately. Headless results do not establish the actual OS appearance,
screen-reader behavior, signing, hosted CI or Cloudflare deployment. Record
unavailable real-window evidence explicitly.

Local Linux evidence for this revision: 33 native interaction/unit tests and
native Clippy pass; the application was launched through GTK/X11 and its own
1120×860-requested window was captured at the host-constrained 1280×720 size.
`docs/screenshots/native-desktop.png` shows the disconnected light appearance.
The session-bus and ibus modules were unavailable in that environment. This
verifies a real app-window render only, not remote capture/input, screen-reader
operation, Windows/macOS runtime behavior or complete light/dark visual parity.
The Web production screenshots include pointer help and a touch-expanded help
panel; see `docs/screenshots/README.md` and `npm --prefix web run screenshots`.
