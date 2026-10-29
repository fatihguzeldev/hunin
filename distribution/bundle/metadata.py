"""Record measured host and explicit build provenance without inventing compatibility."""

import os
import platform
import subprocess


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
