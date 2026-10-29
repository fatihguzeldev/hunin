#!/usr/bin/env python3
"""Install a checksum-pinned official runtime into an explicit local directory."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import stat
import tarfile
import tempfile
import urllib.request
import zipfile

MANIFEST = json.loads(Path(__file__).with_name("runtime-assets.json").read_text())
PLATFORMS = ("linux-x64", "linux-arm64", "macos-x64", "macos-arm64", "windows-x64")


def digest(path):
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def asset(runtime, platform):
    release = MANIFEST["runtimes"][runtime]
    if platform not in release["platforms"]:
        missing = release.get("unavailable", {}).get(platform, {})
        raise ValueError(
            f"{runtime} on {platform}: {missing.get('reason', 'no pinned official archive')} See {missing.get('source', release['source'])}"
        )
    return release["platforms"][platform]


def unpack(archive, destination):
    total = 0
    if archive.suffix == ".zip":
        with zipfile.ZipFile(archive) as bundle:
            for entry in bundle.infolist():
                mode = entry.external_attr >> 16
                path = destination / entry.filename
                if (
                    "\\" in entry.filename
                    or Path(entry.filename).is_absolute()
                    or not path.resolve().is_relative_to(destination.resolve())
                    or stat.S_ISLNK(mode)
                ):
                    raise ValueError(f"Unsafe runtime archive path: {entry.filename}")
                total += entry.file_size
                if total > 2 * 1024**3:
                    raise ValueError("Runtime archive exceeds the unpacked size limit")
                if entry.is_dir():
                    path.mkdir(parents=True, exist_ok=True)
                else:
                    path.parent.mkdir(parents=True, exist_ok=True)
                    with bundle.open(entry) as source, path.open("wb") as output:
                        shutil.copyfileobj(source, output)
                    path.chmod(0o755 if mode & 0o111 else 0o644)
    else:
        with tarfile.open(archive) as bundle:
            for entry in bundle.getmembers():
                if "\\" in entry.name or Path(entry.name).is_absolute():
                    raise ValueError(f"Unsafe runtime archive path: {entry.name}")
                total += entry.size
                if total > 2 * 1024**3:
                    raise ValueError("Runtime archive exceeds the unpacked size limit")
            # The data filter rejects device files and links outside the root,
            # while retaining internal library/launcher links needed by Python.
            bundle.extractall(destination, filter="data")


def tree_digest(directory):
    result = hashlib.sha256()
    for path in sorted(directory.rglob("*")):
        if path == directory / "receipt.json" or (
            path.is_dir() and not path.is_symlink()
        ):
            continue
        relative = path.relative_to(directory).as_posix()
        value = f"link:{os.readlink(path)}" if path.is_symlink() else digest(path)
        result.update(f"{relative}\0{value}\n".encode())
    return result.hexdigest()


def install(runtime, platform, destination, archive_path=None):
    selected = asset(runtime, platform)
    destination = destination.resolve()
    receipt = destination / "receipt.json"
    if destination.exists():
        if receipt.is_file():
            saved = json.loads(receipt.read_text())
            binary = destination / saved.get("binary", "missing")
            if (
                binary.resolve().is_relative_to(destination)
                and saved.get("archive_sha256") == selected["sha256"]
                and binary.is_file()
                and tree_digest(destination) == saved.get("tree_sha256")
            ):
                return binary
        raise ValueError(f"Preserving existing runtime directory: {destination}")
    destination.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(
        prefix=f".{runtime}-", dir=destination.parent
    ) as temporary:
        work = Path(temporary)
        archive = archive_path or work / selected["url"].rsplit("/", 1)[-1]
        if not archive_path:
            request = urllib.request.Request(
                selected["url"], headers={"User-Agent": "hunin-runtime-installer"}
            )
            with (
                urllib.request.urlopen(request, timeout=60) as source,
                archive.open("wb") as output,
            ):
                size = 0
                while chunk := source.read(1024 * 1024):
                    size += len(chunk)
                    if size > 512 * 1024**2:
                        raise ValueError(
                            "Runtime archive exceeds the download size limit"
                        )
                    output.write(chunk)
        if digest(archive) != selected["sha256"]:
            raise ValueError("Official runtime archive checksum mismatch")
        unpacked = work / "unpacked"
        unpacked.mkdir()
        unpack(archive, unpacked)
        candidates = [
            p
            for p in unpacked.rglob(selected["executable"])
            if p.is_file() and not p.is_symlink()
        ]
        # Some Python archives include a launcher in both bin/ and libexec/.
        # Prefer the public bin/ launcher, or the unique archive-root binary.
        public = [p for p in candidates if p.parent.name == "bin"]
        candidates = public or candidates
        if len(candidates) != 1:
            raise ValueError(
                f"Runtime archive has no unique public {selected['executable']} executable: {candidates}"
            )
        binary = candidates[0]
        binary.chmod(0o755)
        relative = binary.relative_to(unpacked)
        receipt_value = {
            "runtime": runtime,
            "version": MANIFEST["runtimes"][runtime]["version"],
            "platform": platform,
            "url": selected["url"],
            "archive_sha256": selected["sha256"],
            "binary": relative.as_posix(),
            "tree_sha256": tree_digest(unpacked),
        }
        (unpacked / "receipt.json").write_text(
            json.dumps(receipt_value, indent=2) + "\n"
        )
        unpacked.rename(destination)
    return destination / relative


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("runtime", choices=MANIFEST["runtimes"])
    parser.add_argument("--platform", choices=PLATFORMS, required=True)
    parser.add_argument("--destination", type=Path, required=True)
    parser.add_argument(
        "--archive",
        type=Path,
        help="reuse a local archive after verifying the pinned checksum",
    )
    parser.add_argument(
        "--github-env",
        action="store_true",
        help="publish HUNIN_TEST_<RUNTIME> to this GitHub Actions job",
    )
    args = parser.parse_args()
    if args.github_env and not os.environ.get("GITHUB_ENV"):
        parser.error("--github-env requires a GitHub Actions job environment")
    binary = install(args.runtime, args.platform, args.destination, args.archive)
    if args.github_env:
        with open(os.environ["GITHUB_ENV"], "a", encoding="utf-8") as environment:
            environment.write(f"HUNIN_TEST_{args.runtime.upper()}={binary}\n")
    print(binary)


if __name__ == "__main__":
    main()
