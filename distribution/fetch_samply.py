#!/usr/bin/env python3
"""Install the pinned official samply release without changing OS profiler permissions."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import tarfile
import tempfile
import urllib.request
import zipfile

VERSION = "0.13.1"
ASSETS = {
    "macos-arm64": (
        "samply-aarch64-apple-darwin.tar.xz",
        "7597239aa3769e75058be5ed359dbfe067f5e7714a5f052c45dd81d509aec17f",
    ),
    "macos-x64": (
        "samply-x86_64-apple-darwin.tar.xz",
        "a57f05f9162d06c36df6d51425d976ac774a8cd6dbd84050e6303fa8bf813998",
    ),
    "linux-arm64": (
        "samply-aarch64-unknown-linux-gnu.tar.xz",
        "aa465162b62830168775b7ff4804bc35049436dcbc29bb3d1ea9f580380ea06a",
    ),
    "linux-x64": (
        "samply-x86_64-unknown-linux-gnu.tar.xz",
        "61875daad67888798690dea3cb2748279df6ac299c5c6a857d67eed7642473d9",
    ),
    "windows-x64": (
        "samply-x86_64-pc-windows-msvc.zip",
        "8fed74dac18197bbb2520125b0199255e09709bed7903a7672a06225a5d7e976",
    ),
}


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def unpack(archive, destination):
    """Copy only regular archive files after verifying the whole release digest."""
    if archive.suffix == ".zip":
        with zipfile.ZipFile(archive) as bundle:
            entries = [
                (
                    entry.filename,
                    entry.file_size,
                    lambda entry=entry: bundle.open(entry),
                )
                for entry in bundle.infolist()
                if not entry.is_dir()
            ]
            copy_entries(entries, destination)
    else:
        with tarfile.open(archive) as bundle:
            entries = [
                (entry.name, entry.size, lambda entry=entry: bundle.extractfile(entry))
                for entry in bundle.getmembers()
                if entry.isfile()
            ]
            copy_entries(entries, destination)


def copy_entries(entries, destination):
    total = 0
    for name, size, opener in entries:
        path = destination / name
        if "\\" in name or not path.resolve().is_relative_to(destination.resolve()):
            raise ValueError(f"Unsafe release archive path: {name}")
        total += size
        if total > 256 * 1024 * 1024:
            raise ValueError("Release archive exceeds the unpacked size limit")
        path.parent.mkdir(parents=True, exist_ok=True)
        with opener() as source, path.open("wb") as target:
            shutil.copyfileobj(source, target)


def install(target, destination):
    name, checksum = ASSETS[target]
    destination = destination.resolve()
    receipt = destination / "receipt.json"
    if destination.exists():
        if receipt.is_file():
            saved = json.loads(receipt.read_text())
            binary = destination / saved.get("binary", "missing")
            if (
                binary.resolve().is_relative_to(destination)
                and saved.get("archive_sha256") == checksum
                and binary.is_file()
                and digest(binary) == saved.get("binary_sha256")
            ):
                return binary
        raise ValueError(f"Preserving existing tool directory: {destination}")
    destination.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(
        prefix=".samply-", dir=destination.parent
    ) as temporary:
        work = Path(temporary)
        archive = work / name
        request = urllib.request.Request(
            f"https://github.com/mstange/samply/releases/download/samply-v{VERSION}/{name}",
            headers={"User-Agent": "hunin-prerequisite-installer"},
        )
        with (
            urllib.request.urlopen(request, timeout=60) as source,
            archive.open("wb") as output,
        ):
            size = 0
            while chunk := source.read(1024 * 1024):
                size += len(chunk)
                if size > 64 * 1024 * 1024:
                    raise ValueError("Release archive exceeds the download size limit")
                output.write(chunk)
        if digest(archive) != checksum:
            raise ValueError("Official release archive checksum mismatch")
        unpacked = work / "unpacked"
        unpacked.mkdir()
        unpack(archive, unpacked)
        binaries = list(
            unpacked.rglob("samply.exe" if target.startswith("windows") else "samply")
        )
        if len(binaries) != 1 or not binaries[0].is_file():
            raise ValueError("Release archive has no unique samply executable")
        binary = binaries[0]
        binary.chmod(0o755)
        (unpacked / "receipt.json").write_text(
            json.dumps(
                {
                    "version": VERSION,
                    "platform": target,
                    "archive_sha256": checksum,
                    "binary": binary.relative_to(unpacked).as_posix(),
                    "binary_sha256": digest(binary),
                },
                indent=2,
            )
            + "\n"
        )
        relative = binary.relative_to(unpacked)
        unpacked.rename(destination)
    return destination / relative


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--platform", choices=ASSETS, required=True)
    parser.add_argument("--destination", type=Path, required=True)
    parser.add_argument(
        "--github-env",
        action="store_true",
        help="append HUNIN_SAMPLY to the current GitHub Actions job environment",
    )
    args = parser.parse_args()
    if args.github_env and not os.environ.get("GITHUB_ENV"):
        parser.error("--github-env requires a GitHub Actions job environment")
    binary = install(args.platform, args.destination)
    if args.github_env:
        with open(os.environ["GITHUB_ENV"], "a", encoding="utf-8") as environment:
            environment.write(f"HUNIN_SAMPLY={binary}\n")
    print(binary)
