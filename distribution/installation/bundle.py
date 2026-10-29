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
import subprocess
import tempfile
import threading
import time

CONTRACT_FILES = {
    "record": "protocol/record.schema.json",
    "responses": "protocol/response.schema.json",
    "operations": "protocol/operations.json",
    "native_manifest": "collectors/native/manifest.json",
    "runtime_inventory": "bundle-assets.json",
}
PROBE_OUTPUT_LIMIT = 64 * 1024


def compiled_platform(target):
    platforms = {
        ("linux", "x86_64"): "linux-x64",
        ("linux", "aarch64"): "linux-arm64",
        ("macos", "x86_64"): "macos-x64",
        ("macos", "aarch64"): "macos-arm64",
        ("windows", "x86_64"): "windows-x64",
    }
    if not isinstance(target, dict):
        raise ValueError("Invalid product compiled target")
    platform_name = platforms.get((target.get("os"), target.get("arch")))
    if platform_name is None:
        raise ValueError("Unsupported product compiled target")
    return platform_name


def check_product(
    info,
    version,
    files,
    read,
    target=None,
    require_target=False,
    require_runtime_inventory=False,
):
    """Validate declared executable contracts against already-hashed files."""
    try:
        if (
            not isinstance(info, dict)
            or type(info["schema_version"]) is not int
            or info["schema_version"] != 1
        ):
            raise ValueError("Unsupported product descriptor schema")
        app = info["app"]
        if "target" in app:
            if target is not None and compiled_platform(app["target"]) != target:
                raise ValueError(
                    "Product compiled target does not match package platform"
                )
        elif require_target:
            raise ValueError(
                "Product descriptor has no compiled target; rebuild the binary before packaging"
            )
        if (
            app["name"] != "hunin"
            or app["version"] != version
            or not (
                app["source_revision"] is None
                or isinstance(app["source_revision"], str)
            )
        ):
            raise ValueError("Product identity does not match package version")
        maximum = info["storage"]["maximum_schema_version"]
        if type(maximum) is not int or maximum < 0:
            raise ValueError("Invalid product storage contract")
        for name, path in CONTRACT_FILES.items():
            if name == "runtime_inventory" and name not in info["contracts"]:
                if require_runtime_inventory:
                    raise ValueError(
                        "Product has no runtime inventory binding; rebuild the binary before packaging"
                    )
                # Historical installed versions keep their declared contracts.
                continue
            contract = info["contracts"][name]
            if contract["sha256"] != files[path]["sha256"]:
                raise ValueError(
                    f"Product contract hash mismatch: {path}. Rebuild the binary and its matching assets."
                )
            if name in ("record", "responses") and (
                not isinstance(contract["id"], str)
                or not contract["id"]
                or contract["id"] != json.loads(read(path))["$id"]
            ):
                raise ValueError(f"Product contract schema identity mismatch: {path}")
        native = json.loads(read(CONTRACT_FILES["native_manifest"]))
        declared = info["contracts"]["native_manifest"]
        extension = declared["extension"]
        if (
            type(extension["schema_version"]) is not int
            or not isinstance(extension["flag"], str)
            or declared["version"] != native["version"]
            or json.dumps(extension, sort_keys=True)
            != json.dumps(native["extension"], sort_keys=True)
        ):
            raise ValueError("Product native extension contract mismatch")
    except (KeyError, TypeError, AttributeError, ValueError) as error:
        raise ValueError(f"Invalid product contract descriptor: {error}") from error


def _probe_output(command, work, environment, timeout=10):
    """Drain both pipes under one byte budget and always reap our child."""
    deadline = time.monotonic() + timeout
    process = subprocess.Popen(
        command,
        cwd=work,
        env=environment,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        bufsize=0,
    )
    outputs = [bytearray(), bytearray()]
    lock = threading.Lock()
    exceeded = threading.Event()
    total = 0

    def drain(stream, output):
        nonlocal total
        try:
            while chunk := stream.read(4096):
                with lock:
                    remaining = max(0, PROBE_OUTPUT_LIMIT - total)
                    output.extend(chunk[:remaining])
                    total += len(chunk)
                    if total > PROBE_OUTPUT_LIMIT:
                        exceeded.set()
                if exceeded.is_set():
                    try:
                        process.kill()
                    except OSError:
                        pass
                    break
        finally:
            stream.close()

    readers = [
        threading.Thread(target=drain, args=(stream, output), daemon=True)
        for stream, output in zip((process.stdout, process.stderr), outputs)
    ]
    for reader in readers:
        reader.start()
    try:
        process.wait(timeout=max(0, deadline - time.monotonic()))
        for reader in readers:
            reader.join(timeout=max(0, deadline - time.monotonic()))
        if exceeded.is_set():
            raise ValueError(
                "Executable descriptor output exceeded the 64 KiB combined stdout/stderr byte limit"
            )
        if any(reader.is_alive() for reader in readers):
            raise subprocess.TimeoutExpired(command, timeout)
        return process.returncode, bytes(outputs[0]), bytes(outputs[1])
    except subprocess.TimeoutExpired as error:
        raise ValueError(
            f"Executable descriptor timed out after {timeout:g} seconds"
        ) from error
    finally:
        if process.poll() is None:
            process.kill()
        process.wait(timeout=1)
        for reader in readers:
            reader.join(timeout=1)


def probe_binary(executable):
    """Ask only the data-free descriptor command from an isolated directory."""
    environment = dict(os.environ)
    for key in ("HUNIN_DATA_DIR", "HUNIN_ASSETS_DIR", "HUNIN_UI_DIR", "HUNIN_SAMPLY"):
        environment.pop(key, None)
    try:
        with tempfile.TemporaryDirectory(prefix="hunin-build-info-") as work:
            status, stdout, stderr = _probe_output(
                [str(executable.resolve()), "build-info"], work, environment
            )
        if status:
            raise ValueError(
                f"Executable descriptor command failed ({status}): {stderr[-2000:].decode('utf-8', errors='replace')}"
            )
        return json.loads(stdout)
    except (OSError, subprocess.TimeoutExpired, ValueError) as error:
        raise ValueError(
            f"Cannot read executable product descriptor; rebuild a matching Hunin binary. {error}"
        ) from error


def verify_executable(bundle, manifest):
    if "product_build_info" not in manifest:
        return "legacy_unknown"
    executable = bundle / (
        "hunin.exe" if manifest["platform"].startswith("windows") else "hunin"
    )
    actual = probe_binary(executable)
    check_product(
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
    return component_status(manifest)


def component_status(manifest):
    if "product_build_info" not in manifest:
        return "legacy_unknown"
    if "target" not in manifest["product_build_info"]["app"]:
        return "legacy_unknown_target"
    return "declared_contracts_verified"


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
        check_product(
            manifest["product_build_info"],
            manifest["version"],
            manifest["files"],
            lambda name: (bundle / name).read_bytes(),
            target=manifest["platform"],
        )
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
    compatibility = verify_executable(directory, manifest)
    _switch_link(executable, prefix, bin_dir)
    return {
        "executable": str(executable),
        "component_compatibility": compatibility,
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
        verify_executable(staged, staged_manifest)
        staged.rename(destination)
    executable = destination / executable.name
    if bin_dir:
        _switch_link(executable, prefix, bin_dir)
    return executable


def main(bundle_root=None):
    parser = argparse.ArgumentParser(
        description=__doc__,
        epilog="Version directories are immutable. Activation selects future invocations only; it does not stop running processes, open recording data, or reverse database migrations. Windows uses the printed versioned executable path.",
    )
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
            elif selected["component_compatibility"] == "legacy_unknown_target":
                print(
                    "Legacy product descriptor: compiled target identity is unknown; component hashes and declared host were checked, not a target compatibility guarantee."
                )
            else:
                print(
                    "Declared component contract hashes verified; no recording data was opened."
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
            installed_manifest = json.loads(
                (executable.parent / "manifest.json").read_text()
            )
            status = component_status(installed_manifest)
            if status == "declared_contracts_verified":
                print(
                    "Declared component contract hashes verified; no recording data was opened."
                )
            elif status == "legacy_unknown_target":
                print(
                    "Legacy product descriptor: compiled target identity is unknown; no target compatibility guarantee is inferred."
                )
            else:
                print(
                    "Legacy installation: component compatibility is unknown; only manifest integrity and host identity were verified."
                )
    except (OSError, ValueError) as error:
        parser.exit(1, f"Installation failed: {error}\n")


if __name__ == "__main__":
    main()
