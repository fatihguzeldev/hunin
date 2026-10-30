"""Native component archive preflight: shipped bytes, pinned patches and provenance."""

import hashlib
import json

from . import inventory

RUST_TARGETS = {
    "linux-x64": "x86_64-unknown-linux-gnu",
    "linux-arm64": "aarch64-unknown-linux-gnu",
    "macos-x64": "x86_64-apple-darwin",
    "macos-arm64": "aarch64-apple-darwin",
    "windows-x64": "x86_64-pc-windows-msvc",
}


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def verify_collector(files, target):
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


def verify_patch_inventory(root, files):
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
