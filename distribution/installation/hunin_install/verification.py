"""Bundle integrity, target compatibility and staged executable verification."""

import hashlib
import json
import platform
import re

from . import contracts, probe


def verify_executable(bundle, manifest):
    if "product_build_info" not in manifest:
        return "legacy_unknown"
    executable = bundle / (
        "hunin.exe" if manifest["platform"].startswith("windows") else "hunin"
    )
    actual = probe.probe_binary(executable)
    contracts.check_product(
        actual,
        manifest["version"],
        manifest["files"],
        lambda name: (bundle / name).read_bytes(),
        target=manifest["platform"],
    )
    if json.dumps(actual, sort_keys=True) != json.dumps(
        manifest["product_build_info"], sort_keys=True
    ):
        raise ValueError(
            "Staged executable descriptor differs from the declared product descriptor"
        )
    return contracts.component_status(manifest)


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
    if "product_build_info" in manifest:
        contracts.check_product(
            manifest["product_build_info"],
            manifest["version"],
            manifest["files"],
            lambda name: (bundle / name).read_bytes(),
            target=manifest["platform"],
        )
    return manifest
