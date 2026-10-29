#!/usr/bin/env python3
"""Verify an extracted bundle, then install it into a user-owned directory."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shutil
import tempfile


def host_platform():
    """Identify a selected OS/architecture; this does not establish an OS floor."""
    system = {"Linux": "linux", "Darwin": "macos", "Windows": "windows"}.get(
        platform.system()
    )
    machine = platform.machine().lower()
    architecture = {
        "x86_64": "x64",
        "amd64": "x64",
        "x64": "x64",
        "aarch64": "arm64",
        "arm64": "arm64",
    }.get(machine)
    target = f"{system}-{architecture}"
    if target not in (
        "linux-x64",
        "linux-arm64",
        "macos-x64",
        "macos-arm64",
        "windows-x64",
    ):
        raise ValueError(
            f"Unsupported installation host: {platform.system()} {machine}"
        )
    return target


def verify_host(manifest):
    host = host_platform()
    if manifest["platform"] != host:
        raise ValueError(
            f"Package target {manifest['platform']} does not match installation host {host}"
        )


def verify(bundle):
    manifest = json.loads((bundle / "manifest.json").read_text())
    if manifest.get("schema_version") != 1:
        raise ValueError("Unsupported package manifest")
    if manifest.get("platform") not in (
        "linux-x64",
        "linux-arm64",
        "macos-x64",
        "macos-arm64",
        "windows-x64",
    ) or not re.fullmatch(
        r"[0-9]+\.[0-9]+\.[0-9]+(?:[-+][A-Za-z0-9.-]+)?", manifest.get("version", "")
    ):
        raise ValueError("Invalid package platform or version")
    if not isinstance(manifest.get("files"), dict) or not manifest["files"]:
        raise ValueError("Package manifest has no files")
    for name, expected in manifest["files"].items():
        path = bundle / name
        if (
            "\\" in name
            or path.is_symlink()
            or not path.resolve().is_relative_to(bundle.resolve())
        ):
            raise ValueError(f"Unsafe package path: {name}")
        data = path.read_bytes()
        if (
            len(data) != expected["size"]
            or hashlib.sha256(data).hexdigest() != expected["sha256"]
        ):
            raise ValueError(f"Package checksum mismatch: {name}")
    executable = "hunin.exe" if manifest["platform"].startswith("windows") else "hunin"
    if executable not in manifest["files"] or "ui/index.html" not in manifest["files"]:
        raise ValueError("Package manifest is missing its executable or UI")
    return manifest


def owned_link(link, prefix):
    """Only replace a link to an intact, versioned installation we own."""
    if not link.is_symlink():
        return False
    try:
        executable = link.resolve(strict=True)
        manifest = verify(executable.parent)
        expected = (
            prefix.resolve() / f"{manifest['version']}-{manifest['platform']}" / "hunin"
        )
        return executable == expected and not manifest["platform"].startswith("windows")
    except (OSError, ValueError, KeyError, TypeError):
        return False


def _switch_link(executable, prefix, bin_dir):
    """Publish only the next-launch link; version directories remain immutable."""
    executable = executable.resolve(strict=True)
    bin_dir.mkdir(parents=True, exist_ok=True)
    link = bin_dir / "hunin"
    if (link.exists() or link.is_symlink()) and not owned_link(link, prefix):
        raise ValueError(
            f"Preserving existing executable: {link}. Bundle available at {executable}."
        )
    with tempfile.TemporaryDirectory(prefix=".hunin-activate-", dir=bin_dir) as stage:
        replacement = Path(stage) / "hunin"
        replacement.symlink_to(executable)
        replacement.replace(link)


def activate(prefix, version, bin_dir):
    """Select an intact installed version. No process is stopped or data opened."""
    host = host_platform()
    if host.startswith("windows"):
        raise ValueError(
            "Windows activation uses the versioned executable path; --bin-dir activation is Unix-only"
        )
    if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+(?:[-+][A-Za-z0-9.-]+)?", version):
        raise ValueError("Invalid installed version")
    prefix = prefix.resolve()
    directory = prefix / f"{version}-{host}"
    if directory.is_symlink():
        raise ValueError(f"Preserving existing installation symlink: {directory}")
    manifest = verify(directory)
    verify_host(manifest)
    if manifest["version"] != version:
        raise ValueError("Installed directory does not match its declared version")
    executable = directory / "hunin"
    _switch_link(executable, prefix, bin_dir)
    return {
        "executable": str(executable),
        "component_compatibility": "legacy_unknown",
    }


def install(bundle, prefix, bin_dir=None):
    manifest = verify(bundle)
    verify_host(manifest)
    if bin_dir and manifest["platform"].startswith("windows"):
        raise ValueError(
            "--bin-dir activation is Unix-only; Windows uses the versioned executable path"
        )
    destination = prefix / f"{manifest['version']}-{manifest['platform']}"
    if destination.exists() or destination.is_symlink():
        raise ValueError(
            f"Install directory already exists: {destination}. Choose another --prefix to preserve it."
        )
    prefix.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=".hunin-install-", dir=prefix) as staging:
        staged = Path(staging) / "bundle"
        staged.mkdir()
        for name in [*manifest["files"], "manifest.json"]:
            source = bundle / name
            target = staged / name
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(source, target)
        # The staged bytes own the publication decision. Checking only the
        # source before copying would miss a changed source or copy failure.
        staged_manifest = verify(staged)
        verify_host(staged_manifest)
        executable = staged / (
            "hunin.exe" if manifest["platform"].startswith("windows") else "hunin"
        )
        if executable.name == "hunin":
            executable.chmod(0o755)
        staged.rename(destination)
    executable = destination / executable.name
    if bin_dir:
        _switch_link(executable, prefix, bin_dir)
    return executable


def main(bundle_root=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--prefix",
        type=Path,
        default=Path(os.environ.get("LOCALAPPDATA", Path.home() / ".local/share"))
        / "hunin",
    )
    parser.add_argument(
        "--bin-dir",
        type=Path,
        help="Unix only: directory for a hunin symlink; no PATH modification",
    )
    parser.add_argument(
        "--activate",
        metavar="VERSION",
        help="Unix only: select an intact version already under --prefix for new invocations; running processes and recording data are unchanged",
    )
    args = parser.parse_args()
    if os.name == "nt" and args.bin_dir:
        parser.error(
            "--bin-dir is Unix-only; add the printed executable directory to your user PATH"
        )
    prefix = args.prefix.expanduser().resolve()
    bin_dir = args.bin_dir.expanduser().resolve() if args.bin_dir else None
    if args.activate and not bin_dir:
        parser.error(
            "--activate requires --bin-dir; Windows uses the versioned executable path directly"
        )
    try:
        if args.activate:
            selected = activate(prefix, args.activate, bin_dir)
            print(
                f"Activated {selected['executable']} for new invocations. Running processes and recording data were not changed."
            )
            if selected["component_compatibility"] == "legacy_unknown":
                print(
                    "Legacy installation: component compatibility is unknown; only manifest integrity and host identity were verified. The executable must accept its recording data schema."
                )
        else:
            executable = install(
                bundle_root or Path(__file__).resolve().parent,
                prefix,
                bin_dir,
            )
            print(
                f"Installed {executable}\nAdd {args.bin_dir or executable.parent} to your PATH, or invoke the executable directly. Version directories are immutable; choose another prefix for a different build of the same version."
            )
    except (OSError, ValueError) as error:
        parser.exit(1, f"Installation failed: {error}\n")


if __name__ == "__main__":
    main()
