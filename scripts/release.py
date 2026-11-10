#!/usr/bin/env python3
"""Package already-built artifacts with the Python standard library (Python 3.11+)."""
from __future__ import annotations

import argparse
import hashlib
import io
from pathlib import Path
import re
import sys
import tarfile
import tomllib
import zipfile

TARGETS = (
    "x86_64-pc-windows-msvc",
    "x86_64-apple-darwin",
    "aarch64-apple-darwin",
    "x86_64-unknown-linux-gnu",
    "aarch64-unknown-linux-gnu",
)
COMPONENTS = ("roc-remote", "roc-remote-headless", "roc-remote-native", "roc-remote-relay")
NOTICE = """roc-remote unsigned distribution

These binaries are not signed by the publisher. Windows builds have no
Authenticode signature. macOS builds are not Developer ID signed or notarized;
any toolchain ad-hoc signature does not establish publisher identity.
SHA256SUMS detects corruption but does not authenticate a download or publisher.
No detached signatures are produced by this release process.

Desktop CLI: roc-remote (default Cargo features).
Headless CLI: roc-remote built with --no-default-features, in a separate archive.
Native controller: roc-remote-native, built from NSH source.
Relay: roc-remote-relay. Web: compiled browser assets in the web archive.
Linux GNU builds need a compatible glibc. Desktop/native builds additionally
need the system libraries described in docs/distribution.md in the source tree.
"""


def version_name(value: str) -> str:
    if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9.+-]*", value):
        raise argparse.ArgumentTypeError("version must be a safe archive name, such as v0.1.0")
    return value


def archive_name(component: str, version: str, target: str) -> str:
    suffix = ".zip" if "windows" in target else ".tar.gz"
    return f"{component}-{version}-{target}{suffix}"


def write_archive(destination: Path, root: str, files: list[tuple[Path, str]], executable: bool = False) -> None:
    """Use a single root directory so extraction never spills files beside it."""
    destination.parent.mkdir(parents=True, exist_ok=True)
    repo=Path(__file__).resolve().parent.parent
    files=list(files)+[(repo/name,name) for name in ("LICENSE","THIRD_PARTY.md","README.md","README.zh-CN.md","docs/platforms.md") if (repo/name).is_file()]
    files += [(repo / "assets" / "branding" / name, f"branding/{name}") for name in ("icon.png", "icon.ico", "README.md")]
    files += [(repo / "assets" / "icons" / "LICENSE", "assets/icons/LICENSE")]
    notice_path = f"{root}/UNSIGNED.txt"
    if destination.suffix == ".zip":
        with zipfile.ZipFile(destination, "w", compression=zipfile.ZIP_DEFLATED) as archive:
            archive.writestr(notice_path, NOTICE)
            for source, name in files:
                archive.write(source, f"{root}/{name}")
    else:
        with tarfile.open(destination, "w:gz") as archive:
            notice = tarfile.TarInfo(notice_path)
            data = NOTICE.encode()
            notice.size = len(data)
            notice.mode = 0o644
            archive.addfile(notice, io.BytesIO(data))
            for source, name in files:
                entry = archive.gettarinfo(str(source), f"{root}/{name}")
                entry.mode = 0o755 if executable and name in ("roc-remote","roc-remote-native","roc-remote-relay") else 0o644
                entry.uid = entry.gid = 0
                entry.uname = entry.gname = ""
                with source.open("rb") as stream:
                    archive.addfile(entry, stream)


def package_binaries(args: argparse.Namespace) -> None:
    extension = ".exe" if "windows" in args.target else ""
    binaries = {
        "roc-remote": (args.binary_dir / f"roc-remote{extension}", f"roc-remote{extension}"),
        "roc-remote-headless": (args.headless_dir / f"roc-remote{extension}", f"roc-remote{extension}"),
        "roc-remote-native": (args.binary_dir / f"roc-remote-native{extension}", f"roc-remote-native{extension}"),
        "roc-remote-relay": (args.binary_dir / f"roc-remote-relay{extension}", f"roc-remote-relay{extension}"),
    }
    for source, _ in binaries.values():
        if not source.is_file():
            raise ValueError(f"missing compiled binary: {source}")
    for component, (source, filename) in binaries.items():
        root = f"{component}-{args.version}-{args.target}"
        destination = args.output / archive_name(component, args.version, args.target)
        write_archive(destination, root, [(source, filename)], executable=True)
        print(destination)


def package_web(args: argparse.Namespace) -> None:
    if not (args.web_dir / "index.html").is_file():
        raise ValueError(f"missing compiled web entry point: {args.web_dir / 'index.html'}")
    files = []
    for path in sorted(args.web_dir.rglob("*")):
        if path.is_symlink():
            raise ValueError(f"web artifact must not contain symlinks: {path}")
        if path.is_file():
            files.append((path, path.relative_to(args.web_dir).as_posix()))
    root = f"roc-remote-web-{args.version}"
    destination = args.output / f"{root}.tar.gz"
    write_archive(destination, root, files)
    print(destination)


def checksums(args: argparse.Namespace) -> None:
    expected = {
        archive_name(component, args.version, target)
        for target in TARGETS
        for component in COMPONENTS
    }
    expected.add(f"roc-remote-web-{args.version}.tar.gz")
    actual = {path.name for path in args.directory.iterdir() if path.is_file()}
    missing = expected - actual
    unexpected = actual - expected - {"UNSIGNED.txt", "SHA256SUMS"}
    if missing or unexpected:
        raise ValueError(f"release assets differ: missing={sorted(missing)}, unexpected={sorted(unexpected)}")
    (args.directory / "UNSIGNED.txt").write_text(NOTICE, encoding="utf-8")
    lines = []
    for name in sorted(expected | {"UNSIGNED.txt"}):
        with (args.directory / name).open("rb") as stream:
            digest = hashlib.file_digest(stream, "sha256").hexdigest()
        lines.append(f"{digest}  {name}\n")
    (args.directory / "SHA256SUMS").write_text("".join(lines), encoding="utf-8")
    print(f"Checksummed {len(lines)} files in {args.directory}")


def validate_tag(args: argparse.Namespace) -> None:
    # Numeric SemVer and optional prerelease/build fields; no paths or shell syntax.
    if not re.fullmatch(r"v(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-[0-9A-Za-z]+(?:[.-][0-9A-Za-z]+)*)?(?:\+[0-9A-Za-z]+(?:[.-][0-9A-Za-z]+)*)?", args.tag):
        raise ValueError("release tag must have the form vMAJOR.MINOR.PATCH[-PRERELEASE][+BUILD]")
    manifest_path = Path(__file__).resolve().parent.parent / "Cargo.toml"
    manifest = tomllib.loads(manifest_path.read_text(encoding="utf-8"))
    version = manifest["workspace"]["package"]["version"]
    relay_manifest = tomllib.loads((manifest_path.parent / "crates/relay/Cargo.toml").read_text(encoding="utf-8"))
    if relay_manifest["package"].get("version") != version:
        raise ValueError("relay version must stay explicit for standalone Docker builds and match the workspace")
    if args.tag != f"v{version}":
        raise ValueError(f"tag {args.tag!r} does not match workspace version v{version}")
    print(f"Validated {args.tag}")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    binaries = commands.add_parser("binaries", help="archive four compiled binaries for one target")
    binaries.add_argument("--version", required=True, type=version_name)
    binaries.add_argument("--target", required=True, choices=TARGETS)
    binaries.add_argument("--binary-dir", required=True, type=Path)
    binaries.add_argument("--headless-dir", required=True, type=Path)
    binaries.add_argument("--output", type=Path, default=Path("dist"))
    binaries.set_defaults(run=package_binaries)
    web = commands.add_parser("web", help="archive an already-built web artifact")
    web.add_argument("--version", required=True, type=version_name)
    web.add_argument("--web-dir", type=Path, default=Path("web/dist"))
    web.add_argument("--output", type=Path, default=Path("dist"))
    web.set_defaults(run=package_web)
    sums = commands.add_parser("checksums", help="require the complete release matrix, then SHA-256 it")
    sums.add_argument("--directory", type=Path, default=Path("dist"))
    sums.add_argument("--version", required=True, type=version_name)
    sums.set_defaults(run=checksums)
    tag = commands.add_parser("validate-tag", help="require vSEMVER matching the workspace version")
    tag.add_argument("tag")
    tag.set_defaults(run=validate_tag)
    args = parser.parse_args()
    try:
        args.run(args)
    except (ValueError, OSError, tomllib.TOMLDecodeError) as error:
        parser.exit(1, f"release: {error}\n")


if __name__ == "__main__":
    main()
