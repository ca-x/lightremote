# Sources and licenses

The roc-remote eagle logo was supplied and approved by the project owner.
See `assets/branding/README.md` for the original and generated size variants;
it is not sourced from any reference project. Existing copyright notices remain
unchanged by the rebrand.

Workspace and context-help icons use six static Kubai Icons from
`kubai087/kubai-icons`, commit `c583030f168294d0e1926e1e7a4fbcf4ee0676ae`.
The icon definitions and package renderer are MIT-licensed under upstream
`LICENSE-ICONS`; its notice is included at `assets/icons/LICENSE`.
See `assets/icons/README.md` for selection and export settings. The separate
source-available skill and proprietary website are not bundled, and no dynamic
icon runtime or framework dependency is added.

roc-remote is an original MIT-licensed implementation. Architecture references
were read for requirements and platform constraints; their source was not copied:

| Reference | Upstream license | Use |
| --- | --- | --- |
| LightOS Remote Desktop | PolyForm Noncommercial 1.0.0 | Platform and service architecture reference |
| uu-remote-linux | GPL-3.0-or-later | Linux host/service behavior reference; local desktop-session lifetime, backend selection and cancellation reviewed, no source/assets/protocol copied |
| RustDesk | AGPL-3.0 | Primary interaction reference; local desktop/mobile toolbar, focus release and OS permission boundaries reviewed, no source/assets/protocol copied |
| Airhand (`sqlpxc/airhand`) | MIT | README, usage and security documentation reviewed for session-oriented controls and explicit permission feedback; no source, assets or protocol copied |

Direct dependencies are recorded exactly in Cargo.lock and the two npm lockfiles.
Notable dependencies include tunnel-mux (MIT), NSH (Apache-2.0), xcap (MIT),
enigo (MIT), arboard (MIT/Apache-2.0), portable-pty (MIT), cap-std (Apache-2.0
with LLVM exception/MIT), xterm.js (MIT), and Buf's Protobuf runtime
(Apache-2.0/BSD-3-Clause). Consult their packaged license files for authoritative
terms and transitive notices. GTK and other Linux system libraries remain
system dependencies; release binaries do not bundle a Linux desktop runtime.

NSH is pinned to a Git commit in crates/native/Cargo.toml, not an unversioned
system CLI. TMP uses its published tunnel-mux 0.1.0 crate and immutable Node
SDK release archive; no sibling source checkout is needed.

Native UI behavior tests use NSH's `nsh-engine` and `nsh-testing` (Apache-2.0),
pinned to the same commit as the runtime crates. No additional UI or motion
library is bundled.

The 2026-11-10 local reference review used RustDesk (checkout HEAD
`bd9262397208c509ea97af61f0d44621ef25611c`) and uu-remote-linux (checkout HEAD
`fe052cdca7ad90d1ab7c27d031036b1071869070`). These are design references, not
linked dependencies. See `docs/native-ui.md` for reviewed paths and original
implementation decisions. Android packaging uses the same pinned NSH runtime,
Android SDK tools and NDK; it does not incorporate either reference project's
Android implementation.
