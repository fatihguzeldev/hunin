#!/usr/bin/env python3
"""Assemble a host-built Hunin bundle; packaging does not certify collector support."""

import argparse
import gzip
import hashlib
import io
import json
from pathlib import Path
import tarfile
import tomllib
import zipfile

from . import inventory
from .metadata import build_metadata, sdk_versions

from ..installation.hunin_install import contracts, probe

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
            "Build the native collector before packaging: python -m collectors.native.downstream.build --output collectors/native/bin/samply[.exe] --cache /your/build-cache"
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


def verify_native_patch_inventory(root, files):
    base = "collectors/native/"
    manifest = json.loads(files[base + "manifest.json"])
    pattern = inventory.relative(manifest["patches"])
    source = root / base
    expected = {
        base + path.relative_to(source).as_posix() for path in source.glob(pattern)
    }
    selected = {
        name
        for name in files
        if name.startswith(base + "patches/") and name.endswith(".patch")
    }
    if not expected or expected != selected:
        raise ValueError(
            "Native patch inventory does not match the current build manifest; "
            "update the explicit runtime assets and rebuild the native collector. "
            f"Missing: {sorted(expected - selected)}; extra: {sorted(selected - expected)}"
        )


def contents(root, binary, target):
    files = {"hunin.exe" if binary.suffix == ".exe" else "hunin": binary.read_bytes()}
    files.update(inventory.contents(root, target))
    verify_native_patch_inventory(root, files)
    verify_native_collector(files, target)
    return files


def assemble(root, binary, output, target, rust_toolchain=None, rust_target=None):
    version = tomllib.loads((root / "Cargo.toml").read_text())["package"]["version"]
    if (target.startswith("windows")) != (binary.suffix == ".exe"):
        raise ValueError(
            "Windows packages require a .exe; Unix packages require a native binary"
        )
    name = f"hunin-{version}-{target}"
    archive = output / f"{name}{'.zip' if target.startswith('windows') else '.tar.gz'}"
    checksum_path = archive.with_name(archive.name + ".sha256")
    for path in (archive, checksum_path):
        if path.exists() or path.is_symlink():
            raise ValueError(
                f"Preserving existing bundle output: {path}. Choose a fresh --output directory."
            )
    files = contents(root, binary, target)
    product_info = probe.probe_binary(binary)
    file_metadata = {
        name: {"sha256": sha256(data), "size": len(data)}
        for name, data in sorted(files.items())
    }
    contracts.check_product(
        product_info,
        version,
        file_metadata,
        files.__getitem__,
        target=target,
        require_target=True,
        require_runtime_inventory=True,
    )
    manifest = {
        "schema_version": 1,
        "version": version,
        "platform": target,
        **build_metadata(root, rust_toolchain, rust_target, product_info),
        "product_build_info": product_info,
        "sdk_versions": sdk_versions(files),
        "files": file_metadata,
    }
    files["manifest.json"] = (json.dumps(manifest, indent=2) + "\n").encode()
    output.mkdir(parents=True, exist_ok=True)
    if target.startswith("windows"):
        with zipfile.ZipFile(archive, "x", compression=zipfile.ZIP_DEFLATED) as bundle:
            for path, data in sorted(files.items()):
                entry = zipfile.ZipInfo(
                    f"{name}/{path}", date_time=(1980, 1, 1, 0, 0, 0)
                )
                entry.external_attr = 0o644 << 16
                bundle.writestr(entry, data, compress_type=zipfile.ZIP_DEFLATED)
    else:
        with (
            archive.open("xb") as raw,
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
    with archive.open("rb") as stream:
        checksum = hashlib.file_digest(stream, "sha256").hexdigest()
    with checksum_path.open("x") as stream:
        stream.write(f"{checksum}  {archive.name}\n")
    return archive


def main():
    root = Path(__file__).resolve().parents[2]
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
            root,
            args.binary.resolve(),
            args.output.resolve(),
            args.platform,
            args.rust_toolchain,
            args.rust_target,
        )
    )


if __name__ == "__main__":
    main()
