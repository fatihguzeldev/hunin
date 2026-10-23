#!/usr/bin/env python3
"""Build Hunin's checksum-pinned native collector into a distributable binary."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile
import urllib.request

ROOT = Path(__file__).resolve().parent
MANIFEST = json.loads((ROOT / "manifest.json").read_text())


def digest(path):
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def tree_digest(directory):
    result = hashlib.sha256()
    for path in sorted(directory.rglob("*")):
        if path.is_symlink():
            raise ValueError(f"Unexpected symlink in native collector source: {path}")
        if path.is_file():
            result.update(f"{path.relative_to(directory).as_posix()}\0{digest(path)}\n".encode())
    return result.hexdigest()


def unpack(archive, destination):
    total = 0
    with tarfile.open(archive) as bundle:
        for entry in bundle.getmembers():
            path = destination / entry.name
            if "\\" in entry.name or not path.resolve().is_relative_to(destination.resolve()) or not (entry.isfile() or entry.isdir()):
                raise ValueError(f"Unsafe source archive path: {entry.name}")
            total += entry.size
            if total > 256 * 1024**2:
                raise ValueError("Native collector source exceeds the unpacked size limit")
        bundle.extractall(destination, filter="data")
    roots = list(destination.iterdir())
    if len(roots) != 1 or not (roots[0] / "Cargo.lock").is_file():
        raise ValueError("Native collector source has no unique locked workspace")
    return roots[0]


def source_archive(cache, supplied=None):
    expected = MANIFEST["upstream"]["archive_sha256"]
    archive = supplied or cache / f"{expected}.tar.gz"
    if not archive.exists():
        if supplied:
            raise ValueError(f"Source archive does not exist: {archive}")
        with tempfile.NamedTemporaryFile(prefix=".samply-download-", dir=cache, delete=False) as output:
            temporary = Path(output.name)
        try:
            with temporary.open("wb") as output:
                request = urllib.request.Request(MANIFEST["upstream"]["archive_url"], headers={"User-Agent": "hunin-native-collector-build"})
                with urllib.request.urlopen(request, timeout=60) as source:
                    size = 0
                    while chunk := source.read(1024 * 1024):
                        size += len(chunk)
                        if size > 64 * 1024**2:
                            raise ValueError("Native collector source exceeds the download size limit")
                        output.write(chunk)
            if digest(temporary) != expected:
                raise ValueError("Native collector source archive checksum mismatch")
            temporary.replace(archive)
        finally:
            temporary.unlink(missing_ok=True)
    if digest(archive) != expected:
        raise ValueError("Native collector source archive checksum mismatch")
    return archive


def prepare_source(cache, archive, patches):
    identity = {"upstream": MANIFEST["upstream"], "patches": [{"name": path.name, "sha256": hashlib.sha256(data).hexdigest()} for path, data in patches]}
    key = hashlib.sha256(json.dumps(identity, sort_keys=True).encode()).hexdigest()
    workspace = cache / key
    receipt = workspace / "source.json"
    if workspace.exists():
        saved = json.loads(receipt.read_text()) if receipt.is_file() else {}
        source = workspace / "source"
        if saved.get("identity") != identity or not source.is_dir() or tree_digest(source) != saved.get("source_sha256"):
            raise ValueError(f"Preserving modified native collector build cache: {workspace}")
        return source, workspace, identity
    with tempfile.TemporaryDirectory(prefix=".samply-source-", dir=cache) as temporary:
        staged = Path(temporary) / "workspace"
        staged.mkdir()
        unpacked = unpack(archive, staged / "unpacked")
        source = staged / "source"
        unpacked.rename(source)
        (staged / "unpacked").rmdir()
        for index, (path, data) in enumerate(patches):
            normalized = Path(temporary) / f"{index:04d}-{path.name}"
            normalized.write_bytes(data)
            subprocess.run(["git", "apply", "--check", str(normalized)], cwd=source, check=True, timeout=60)
            subprocess.run(["git", "apply", str(normalized)], cwd=source, check=True, timeout=60)
        (staged / "source.json").write_text(json.dumps({"identity": identity, "source_sha256": tree_digest(source)}, indent=2) + "\n")
        staged.rename(workspace)
    return workspace / "source", workspace, identity


def build(output, cache, archive=None, toolchain="stable", test_filters=()):
    output = output.parent.resolve() / output.name
    if output.is_symlink():
        raise ValueError(f"Preserving existing native collector symlink: {output}")
    cache.mkdir(parents=True, exist_ok=True)
    patches = [(path, path.read_bytes().replace(b"\r\n", b"\n")) for path in sorted(ROOT.glob(MANIFEST["patches"]))]
    if not patches:
        raise ValueError("Native collector extension patches are missing")
    archive = source_archive(cache, archive)
    source, workspace, identity = prepare_source(cache, archive, patches)
    compiler = subprocess.check_output(["rustc", f"+{toolchain}", "--version", "--verbose"], text=True, timeout=5).strip()
    host = next((line.removeprefix("host: ") for line in compiler.splitlines() if line.startswith("host: ")), None)
    if not host:
        raise ValueError("Rust compiler did not report a native host target")
    target = workspace / "target"
    patch_sha256 = hashlib.sha256(json.dumps(identity["patches"], sort_keys=True, separators=(",", ":")).encode()).hexdigest()
    environment = dict(os.environ, CARGO_TARGET_DIR=str(target), HUNIN_NATIVE_PATCH_SHA256=patch_sha256)
    # Build the host explicitly; an inherited cross target must not change the bundle.
    subprocess.run(["cargo", f"+{toolchain}", "build", "--locked", "--release", "--target", host, "-p", MANIFEST["cargo_package"]],
                   cwd=source, env=environment, check=True, timeout=1800)
    for test_filter in test_filters:
        subprocess.run(["cargo", f"+{toolchain}", "test", "--locked", "--release", "--target", host, "-p", MANIFEST["cargo_package"], test_filter],
                       cwd=source, env=environment, check=True, timeout=600)
    binary = target / host / "release" / ("samply.exe" if os.name == "nt" else "samply")
    if not binary.is_file():
        raise ValueError(f"Native collector build produced no executable: {binary}")
    help_text = subprocess.check_output([str(binary), "record", "--help"], text=True, timeout=10)
    if MANIFEST["extension"]["flag"] not in help_text:
        raise ValueError("Built native collector does not expose the pinned lifetime extension")
    version = subprocess.check_output([str(binary), "--version"], text=True, timeout=10).strip()
    if version != f"samply {MANIFEST['version']}":
        raise ValueError(f"Built native collector reported an unexpected version: {version}")
    receipt = output.with_name(output.name + ".provenance.json")
    if output.exists():
        previous = json.loads(receipt.read_text()) if receipt.is_file() else {}
        if previous.get("binary_sha256") != digest(output):
            raise ValueError(f"Preserving unrelated or modified native collector executable: {output}")
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(prefix=".samply-publish-", dir=output.parent, delete=False) as temporary:
        staged = Path(temporary.name)
    try:
        shutil.copy2(binary, staged)
        staged.chmod(0o755)
        staged.replace(output)
    finally:
        staged.unlink(missing_ok=True)
    for name in ("LICENSE-MIT", "LICENSE-APACHE"):
        shutil.copy2(source / name, output.parent / name)
    receipt.write_text(json.dumps({"schema_version": 1, "identity": identity, "version": MANIFEST["version"], "extension": MANIFEST["extension"], "patch_sha256": patch_sha256,
                                   "rustc": compiler, "target": host, "binary_sha256": digest(output)}, indent=2) + "\n")
    return output


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--cache", type=Path, required=True)
    parser.add_argument("--archive", type=Path, help="Reuse a local archive after verifying the pinned digest")
    parser.add_argument("--rust-toolchain", default="stable")
    parser.add_argument("--test-filter", action="append", default=[], help="Run a focused source test filter before publishing the collector")
    parser.add_argument("--github-env", action="store_true")
    args = parser.parse_args()
    if args.github_env and not os.environ.get("GITHUB_ENV"):
        parser.error("--github-env requires a GitHub Actions job environment")
    binary = build(args.output.absolute(), args.cache.resolve(), args.archive.resolve() if args.archive else None, args.rust_toolchain, args.test_filter)
    if args.github_env:
        with open(os.environ["GITHUB_ENV"], "a", encoding="utf-8") as environment:
            environment.write(f"HUNIN_SAMPLY={binary}\n")
    print(binary)
