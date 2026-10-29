#!/usr/bin/env python3
"""Assemble a host-built Hunin bundle; packaging does not certify collector support."""

import argparse
import gzip
import hashlib
import io
import json
import os
from pathlib import Path
import platform
import subprocess
import tarfile
import tomllib
import zipfile

if __package__:
    from .bundle import inventory
else:
    from bundle import inventory

PLATFORMS = ("linux-x64", "linux-arm64", "macos-x64", "macos-arm64", "windows-x64")
RUST_TARGETS = {
    "linux-x64": "x86_64-unknown-linux-gnu",
    "linux-arm64": "aarch64-unknown-linux-gnu",
    "macos-x64": "x86_64-apple-darwin",
    "macos-arm64": "aarch64-apple-darwin",
    "windows-x64": "x86_64-pc-windows-msvc",
}


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def command_output(command):
    try:
        result = subprocess.run(command, capture_output=True, text=True, timeout=5)
        return result.stdout.strip() if result.returncode == 0 else None
    except (OSError, subprocess.TimeoutExpired):
        return None


def build_metadata(root, rust_toolchain=None, rust_target=None):
    distribution = {}
    if platform.system() == "Linux":
        try:
            distribution = platform.freedesktop_os_release()
        except OSError:
            pass
    libc_name, libc_version = platform.libc_ver()
    revision = command_output(["git", "-C", str(root), "rev-parse", "HEAD"])
    changes = command_output(["git", "-C", str(root), "status", "--porcelain"])
    verbose = (
        command_output(["rustc", f"+{rust_toolchain}", "--version", "--verbose"])
        if rust_toolchain
        else None
    )
    rust = dict(
        line.split(": ", 1) for line in (verbose or "").splitlines() if ": " in line
    )
    server, repository, run = (
        os.environ.get(name)
        for name in ("GITHUB_SERVER_URL", "GITHUB_REPOSITORY", "GITHUB_RUN_ID")
    )
    run_url = (
        f"{server.rstrip('/')}/{repository}/actions/runs/{run}"
        if all((server, repository, run))
        else "unknown"
    )
    return {
        "built_on": {
            "os": platform.system(),
            "arch": platform.machine(),
            "release": platform.release() or "unknown",
            "version": platform.version() or "unknown",
            "distribution": distribution.get("PRETTY_NAME", "unknown"),
            "libc": {
                "name": libc_name or "unknown",
                "version": libc_version or "unknown",
            },
        },
        "build_provenance": {
            "source_revision": revision or "unknown",
            "source_dirty": changes != "" if changes is not None else "unknown",
            "github_sha": os.environ.get("GITHUB_SHA") or "unknown",
            "workflow_run_url": run_url,
            "rust": {
                "toolchain": rust_toolchain or "unknown",
                "version": rust.get("release", "unknown"),
                "host": rust.get("host", "unknown"),
                "target": rust_target or "unknown",
            },
        },
        "compatibility": {
            "minimum_os": "not_established",
            "validation": "Consult workflow evidence; packaging alone does not establish collector support.",
        },
    }


def verify_native_collector(files, target):
    base = "collectors/native/"
    executable = (
        base + "bin/" + ("samply.exe" if target.startswith("windows") else "samply")
    )
    required = (
        executable,
        executable + ".provenance.json",
        base + "manifest.json",
        base + "bin/LICENSE-MIT",
        base + "bin/LICENSE-APACHE",
    )
    if any(name not in files for name in required):
        raise ValueError(
            "Build the native collector before packaging: python collectors/native/build.py --output collectors/native/bin/samply[.exe] --cache /your/build-cache"
        )
    receipt = json.loads(files[executable + ".provenance.json"])
    manifest = json.loads(files[base + "manifest.json"])
    patches = [
        {
            "name": name.removeprefix(base + "patches/"),
            "sha256": sha256(data.replace(b"\r\n", b"\n")),
        }
        for name, data in sorted(files.items())
        if name.startswith(base + "patches/") and name.endswith(".patch")
    ]
    if (
        not patches
        or receipt.get("binary_sha256") != sha256(files[executable])
        or receipt.get("target") != RUST_TARGETS[target]
        or receipt.get("version") != manifest["version"]
        or receipt.get("extension") != manifest["extension"]
        or receipt.get("patch_sha256")
        != sha256(json.dumps(patches, sort_keys=True, separators=(",", ":")).encode())
        or receipt.get("identity")
        != {"upstream": manifest["upstream"], "patches": patches}
    ):
        raise ValueError(
            "Bundled native collector does not match its build provenance, platform, or pinned patches"
        )


def contents(root, binary, target):
    files = {"hunin.exe" if binary.suffix == ".exe" else "hunin": binary.read_bytes()}
    files.update(inventory.contents(root, target))
    verify_native_collector(files, target)
    return files


def assemble(root, binary, output, target, rust_toolchain=None, rust_target=None):
    version = tomllib.loads((root / "Cargo.toml").read_text())["package"]["version"]
    if (target.startswith("windows")) != (binary.suffix == ".exe"):
        raise ValueError(
            "Windows packages require a .exe; Unix packages require a native binary"
        )
    files = contents(root, binary, target)
    manifest = {
        "schema_version": 1,
        "version": version,
        "platform": target,
        **build_metadata(root, rust_toolchain, rust_target),
        "files": {
            name: {"sha256": sha256(data), "size": len(data)}
            for name, data in sorted(files.items())
        },
    }
    files["manifest.json"] = (json.dumps(manifest, indent=2) + "\n").encode()
    name = f"hunin-{version}-{target}"
    output.mkdir(parents=True, exist_ok=True)
    archive = output / f"{name}{'.zip' if target.startswith('windows') else '.tar.gz'}"
    if target.startswith("windows"):
        with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_DEFLATED) as bundle:
            for path, data in sorted(files.items()):
                entry = zipfile.ZipInfo(
                    f"{name}/{path}", date_time=(1980, 1, 1, 0, 0, 0)
                )
                entry.external_attr = 0o644 << 16
                bundle.writestr(entry, data, compress_type=zipfile.ZIP_DEFLATED)
    else:
        with (
            archive.open("wb") as raw,
            gzip.GzipFile(fileobj=raw, mode="wb", mtime=0, filename="") as compressed,
            tarfile.open(fileobj=compressed, mode="w") as bundle,
        ):
            for path, data in sorted(files.items()):
                entry = tarfile.TarInfo(f"{name}/{path}")
                entry.size = len(data)
                entry.mode = (
                    0o755
                    if path in ("hunin", "install.py", "collectors/native/bin/samply")
                    else 0o644
                )
                entry.mtime = 0
                bundle.addfile(entry, io.BytesIO(data))
    checksum = sha256(archive.read_bytes())
    archive.with_name(archive.name + ".sha256").write_text(
        f"{checksum}  {archive.name}\n"
    )
    return archive


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--platform", choices=PLATFORMS, required=True)
    parser.add_argument("--output", type=Path, default=Path("dist"))
    parser.add_argument(
        "--rust-toolchain",
        help="The Rust toolchain used for this build; omitted provenance stays unknown",
    )
    parser.add_argument(
        "--rust-target",
        help="The target triple used for this build; do not infer it from the bundle name",
    )
    args = parser.parse_args()
    print(
        assemble(
            Path(__file__).resolve().parents[1],
            args.binary.resolve(),
            args.output.resolve(),
            args.platform,
            args.rust_toolchain,
            args.rust_target,
        )
    )
