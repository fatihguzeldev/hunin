#!/usr/bin/env python3
"""Verify an extracted bundle, then install it into a user-owned directory."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import tempfile


def verify(bundle):
    manifest = json.loads((bundle / "manifest.json").read_text())
    if manifest.get("schema_version") != 1:
        raise ValueError("Unsupported package manifest")
    if manifest.get("platform") not in ("linux-x64", "linux-arm64", "macos-x64", "macos-arm64", "windows-x64") or not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+(?:[-+][A-Za-z0-9.-]+)?", manifest.get("version", "")):
        raise ValueError("Invalid package platform or version")
    if not isinstance(manifest.get("files"), dict) or not manifest["files"]:
        raise ValueError("Package manifest has no files")
    for name, expected in manifest["files"].items():
        path = bundle / name
        if "\\" in name or path.is_symlink() or not path.resolve().is_relative_to(bundle.resolve()):
            raise ValueError(f"Unsafe package path: {name}")
        data = path.read_bytes()
        if len(data) != expected["size"] or hashlib.sha256(data).hexdigest() != expected["sha256"]:
            raise ValueError(f"Package checksum mismatch: {name}")
    executable = "hunin.exe" if manifest["platform"].startswith("windows") else "hunin"
    if executable not in manifest["files"] or "ui/index.html" not in manifest["files"]:
        raise ValueError("Package manifest is missing its executable or UI")
    return manifest


def install(bundle, prefix, bin_dir=None):
    manifest = verify(bundle)
    destination = prefix / f"{manifest['version']}-{manifest['platform']}"
    if destination.exists():
        raise ValueError(f"Install directory already exists: {destination}. Choose another --prefix to preserve it.")
    prefix.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=".hunin-install-", dir=prefix) as staging:
        staged = Path(staging) / "bundle"
        staged.mkdir()
        for name in [*manifest["files"], "manifest.json"]:
            source = bundle / name
            target = staged / name
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(source, target)
        executable = staged / ("hunin.exe" if manifest["platform"].startswith("windows") else "hunin")
        if executable.name == "hunin":
            executable.chmod(0o755)
        staged.rename(destination)
    executable = destination / executable.name
    if bin_dir:
        bin_dir.mkdir(parents=True, exist_ok=True)
        link = bin_dir / "hunin"
        if link.exists() and not link.is_symlink():
            raise ValueError(f"Preserving existing executable: {link}. Bundle installed at {executable}.")
        replacement = bin_dir / ".hunin-link"
        if replacement.exists() or replacement.is_symlink():
            raise ValueError(f"Preserving existing staging link: {replacement}")
        replacement.symlink_to(executable)
        replacement.replace(link)
    return executable


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--prefix", type=Path, default=Path(os.environ.get("LOCALAPPDATA", Path.home() / ".local/share")) / "hunin")
    parser.add_argument("--bin-dir", type=Path, help="Unix only: directory for a hunin symlink; no PATH modification")
    args = parser.parse_args()
    if os.name == "nt" and args.bin_dir:
        parser.error("--bin-dir is Unix-only; add the printed executable directory to your user PATH")
    executable = install(Path(__file__).resolve().parent, args.prefix.expanduser().resolve(), args.bin_dir.expanduser().resolve() if args.bin_dir else None)
    print(f"Installed {executable}\nAdd {args.bin_dir or executable.parent} to your PATH, or invoke the executable directly.")
