"""Record measured host and explicit build provenance without inventing compatibility."""

import os
import hashlib
import json
import platform
import subprocess
import tomllib


def command_output(command):
    try:
        result = subprocess.run(command, capture_output=True, text=True, timeout=5)
        return result.stdout.strip() if result.returncode == 0 else None
    except (OSError, subprocess.TimeoutExpired):
        return None


def sdk_versions(files):
    """Report package identities without requiring SDK/app version lockstep."""
    paths = {
        "javascript": "sdk/javascript/package.json",
        "python": "sdk/python/pyproject.toml",
        "rust": "sdk/rust/Cargo.toml",
        "go": "sdk/go/go.mod",
        "c": "sdk/c/CMakeLists.txt",
        "cpp": "sdk/cpp/include/hunin.hpp",
    }
    versions = {}
    for name, path in paths.items():
        data = files[path]
        if name == "javascript":
            value = json.loads(data).get("version")
        elif name in ("python", "rust"):
            document = tomllib.loads(data.decode())
            value = document.get("project" if name == "python" else "package", {}).get(
                "version"
            )
        else:
            value = None
        versions[name] = {
            "declared_version": value if isinstance(value, str) else None,
            "metadata_file": path,
            "metadata_sha256": hashlib.sha256(data).hexdigest(),
        }
    return versions


def build_metadata(root, rust_toolchain=None, rust_target=None, product_info=None):
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
            "packaging_checkout_revision": revision or "unknown",
            "packaging_checkout_dirty": changes != ""
            if changes is not None
            else "unknown",
            "binary_source_revision": product_info["app"]["source_revision"]
            if product_info
            else None,
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
            "sdk_versions": "Independent package versions; shared record contracts and selected conformance controls establish compatibility, not matching package version numbers.",
            "validation": "Consult workflow evidence; packaging alone does not establish collector support.",
        },
    }
