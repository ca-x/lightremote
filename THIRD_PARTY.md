# Sources and licenses

The roc-remote eagle logo was supplied and approved by the project owner.
See `assets/branding/README.md` for the original and generated size variants;
it is not sourced from any reference project. Existing copyright notices remain
unchanged by the rebrand.

roc-remote is an original MIT-licensed implementation. Architecture references
were read for requirements and platform constraints; their source was not copied:

| Reference | Upstream license | Use |
| --- | --- | --- |
| LightOS Remote Desktop | PolyForm Noncommercial 1.0.0 | Platform and service architecture reference |
| uu-remote-linux | GPL-3.0-or-later | Linux host/service behavior reference |
| RustDesk | AGPL-3.0 | Cross-platform remote-desktop architecture reference |
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
