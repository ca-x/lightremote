# CI and distribution

GitHub Actions runs from a standalone checkout of this repository. It does not
check out sibling repositories, inject local Cargo patches, or install an
unpublished SDK. Commit `Cargo.lock` and both npm lockfiles so every `--locked`
Cargo build and `npm ci` command can resolve the same published dependencies.
NSH's source dependency must be pinned to a resolvable upstream revision.
The Dockerfile builds the relay as a standalone crate using its own committed
`crates/relay/Cargo.lock`, copied with the crate source.

## Continuous integration

`.github/workflows/ci.yml` runs for branch pushes and pull requests and is also
called by the tag release workflow. It contains independent Rust, web, and
Cloudflare check jobs. Rust checks use the workspace's **default-members**
(core, services, CLI, relay, updater, and daemon); NSH is compiled separately on each platform.

- Rust: `cargo fmt --all -- --check`, Clippy for all targets with and without
  default features, and tests in both configurations. Clippy denies warnings.
- Web: `npm ci`, `npm run typecheck`, `npm test`, and `npm run build`.
- Cloudflare: `npm ci`, `npm run check` (TypeScript), and `npm test`.
- Platform builds: desktop CLI, headless CLI, relay, native NSH controller, and headless daemon.

`bash scripts/check.sh [rust|web|cloudflare|all]` reproduces the check commands.
Run `npm ci` in each npm project before invoking this script. Browser Playwright
checks and graphical interactive smoke tests are separate from these unit and
build checks; compiled native/desktop artifacts do not prove screen capture or
input control on a live desktop.

The web job uploads `web-dist`. The Rust build workflow downloads it to
`web/dist` and produces separate `release-TARGET` artifacts. The relay Docker
job also downloads the exact same web artifact to `web/dist` before building
`deploy/Dockerfile` with repository root context. No Rust job rebuilds the web
client, and no release job silently substitutes an empty UI.

## Platforms and native prerequisites

Every platform uses its corresponding hosted runner rather than claiming
cross-platform success from a Linux build:

| Platform | Rust target | GitHub runner |
| --- | --- | --- |
| Windows x64 | `x86_64-pc-windows-msvc` | `windows-2022` |
| macOS x64 | `x86_64-apple-darwin` | `macos-15-intel` |
| macOS arm64 | `aarch64-apple-darwin` | `macos-15` |
| Linux x64 | `x86_64-unknown-linux-gnu` | `ubuntu-24.04` |
| Linux arm64 | `aarch64-unknown-linux-gnu` | `ubuntu-24.04-arm` |

Linux jobs install the development packages needed by the desktop CLI's
`xcap` and `enigo` dependencies and NSH: `libclang-dev`, `libxcb1-dev`,
`libxrandr-dev`, `libdbus-1-dev`, `libpipewire-0.3-dev`, `libgbm-dev`,
`libwayland-dev`, `libegl-dev`, `pkg-config`, `libgtk-4-dev`,
`libgstreamer1.0-dev`, `libgstreamer-plugins-base1.0-dev`, and `clang`.
The native build command is `cargo build --locked --release --target TARGET
-p lightremote-native`; it builds source from the committed manifest and lock.
Hosted Windows runners supply MSVC and the Windows SDK; macOS runners supply
Xcode's command-line tools and system frameworks. The native controller
requires macOS 14 or newer. Its NSH crates are pinned to upstream revision
`0e4a20bd41ee72a35515cecbf9a7fba88742d911`.

These are portable archives, not OS installers. Linux builds use GNU/glibc on
Ubuntu 24.04; they are not musl/static binaries and older distributions are not
promised compatible. The Linux native controller requires GTK 4.14 or newer
and GStreamer runtime libraries (`libgtk-4-1`, `libgstreamer1.0-0`, and
`libgstreamer-plugins-base1.0-0` on Ubuntu 24.04). It uses NSH's Rust components without WebView or QuickJS;
no WebKit runtime, external UI bundle, or installed NSH CLI is needed.
The desktop CLI also requires the system capture/input libraries and an actual
supported graphical session; installing build dependencies does not grant
screen-recording, clipboard, or input permissions. macOS requires the user to
grant the relevant Screen Recording and Accessibility permissions.

The headless executable is built separately with:

```sh
cargo build --locked --release --target TARGET --target-dir target/headless \
  -p lightremote --no-default-features
cargo build --locked --release --target TARGET --target-dir target/headless \
  -p lightremote-daemon --no-default-features
```

It has the same executable name, `lightremote` (`lightremote.exe` on Windows),
as the desktop CLI. Extract the desired archive into a separate directory;
choose which version to put on `PATH`.

## Tag release

1. Set the workspace version and commit the source and lockfiles. Run CI.
2. Push a tag such as `v0.1.1` whose entire value matches `v` plus the workspace
   version. Prereleases require a matching version such as `0.2.0-rc.1`.
3. `release.yml` validates the tag, runs all CI checks and the full native
   platform matrix, then publishes the GHCR relay image.
4. Only after those jobs pass does it create a GitHub release containing the
   compiled binaries, web archive, unsigned notice, and checksums.

The next release will produce five archives per platform, named
`COMPONENT-vVERSION-TARGET.tar.gz` (`.zip` for Windows). Components are
`lightremote`, `lightremote-headless`, `lightremote-native`, `lightremote-relay`,
and `lightremote-daemon`. The daemon is built in the isolated headless target
directory with default features disabled; it is a separate resident host service. The native
NSH build is not a renamed CLI. The separate
web archive is `lightremote-web-vVERSION.tar.gz`. All archives have a single
containing directory and an `UNSIGNED.txt` notice. `SHA256SUMS` covers the
complete set of 26 archives and the top-level notice. Packaging fails if an
expected target/component is missing or an unexpected file appears.
The daemon archive also includes its configuration example, service templates and
deployment/update instructions. Existing v0.1.1 remains the original four-product
release with 23 assets; it is not replaced or expanded by this branch.

For a local packaging check, build the executables first, then use:

```sh
python3 scripts/release.py binaries --version dev-check \
  --target x86_64-unknown-linux-gnu \
  --binary-dir target/x86_64-unknown-linux-gnu/release \
  --headless-dir target/headless/x86_64-unknown-linux-gnu/release \
  --daemon-dir target/headless/x86_64-unknown-linux-gnu/release --output dist
python3 scripts/release.py web --version dev-check --web-dir web/dist --output dist
# Run once all five target archives have been collected:
python3 scripts/release.py checksums --version dev-check --directory dist
```

The packager requires Python 3.11 or newer. To check downloaded files on Linux,
run `sha256sum --check SHA256SUMS` in the download directory. On macOS use
`shasum -a 256 --check SHA256SUMS`; on Windows compare `Get-FileHash -Algorithm
SHA256 FILE` against the corresponding line.

**All published downloads are unsigned.** No Authenticode certificate,
Developer ID signature, notarization, or detached cryptographic signature is
created. A toolchain-generated macOS ad-hoc signature does not authenticate the
publisher. Checksums detect modified downloads but do not establish publisher
identity. There is no signing-secret placeholder or signing-success claim.

A release run does not overwrite an existing GitHub release. If publication
fails after the GHCR push, the image can already exist even though the GitHub
release does not. Diagnose the failure before retrying; image tags are mutable.
Protect release tags and limit who can push them. Publishing requires Actions
`contents: write` for the release job and `packages: write` for the GHCR job.
The rest of the workflow has read-only repository permissions.

## Relay container and Cloudflare

The Docker build publishes
`ghcr.io/OWNER/REPOSITORY-relay:VERSION` with the repository path lowercased and
the `v` removed from the version (Docker normalizes `+` build metadata to `_`).
It includes the built web client and has
`linux/amd64` and `linux/arm64` variants. Stable versions also update the
`MAJOR.MINOR` and `latest` image tags; prereleases do not update `latest`.
An additional commit tag records the source revision. Repository settings may
require enabling GHCR publishing, granting this repository access to an
existing package, or changing a newly created package's visibility to public.
No registry password secret is needed: the job uses its scoped `GITHUB_TOKEN`.

The workflow builds `deploy/Dockerfile` with Buildx and QEMU. It does not
provide a production relay token, deploy a container, expose a service, or run
Cloudflare deployment. Follow the relay deployment documentation and supply
operator-owned configuration. Cloudflare's tests run in CI; Worker/Durable
Object deployment remains an explicit operator action using the Cloudflare
project's commands and credentials.

Action references are pinned to commit SHAs resolved from the official
`actions/*` and `docker/*` repositories' major-version tags. Rust stable, Node
22, hosted runner images, and Docker base image tags still receive updates;
these workflows do not claim byte-for-byte reproducible builds or artifact
signing.
