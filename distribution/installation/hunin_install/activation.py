"""Immutable version installation and ownership-checked next-launch activation."""

from pathlib import Path
import re
import shutil
import tempfile

from . import verification


def owned_link(link, prefix):
    """Only replace a link to an intact, versioned installation we own."""
    if not link.is_symlink():
        return False
    try:
        executable = link.resolve(strict=True)
        manifest = verification.verify(executable.parent)
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
    host = verification.host_platform()
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
    manifest = verification.verify(directory)
    verification.verify_host(manifest)
    if manifest["version"] != version:
        raise ValueError("Installed directory does not match its declared version")
    executable = directory / "hunin"
    compatibility = verification.verify_executable(directory, manifest)
    _switch_link(executable, prefix, bin_dir)
    return {
        "executable": str(executable),
        "component_compatibility": compatibility,
    }


def install(bundle, prefix, bin_dir=None):
    manifest = verification.verify(bundle)
    verification.verify_host(manifest)
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
        staged_manifest = verification.verify(staged)
        verification.verify_host(staged_manifest)
        executable = staged / (
            "hunin.exe" if manifest["platform"].startswith("windows") else "hunin"
        )
        if executable.name == "hunin":
            executable.chmod(0o755)
        verification.verify_executable(staged, staged_manifest)
        staged.rename(destination)
    executable = destination / executable.name
    if bin_dir:
        _switch_link(executable, prefix, bin_dir)
    return executable
