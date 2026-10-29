#!/usr/bin/env python3
"""Compatibility command for the retired stock Samply installer."""

import argparse
from pathlib import Path

PLATFORMS = ("linux-x64", "linux-arm64", "macos-x64", "macos-arm64", "windows-x64")


def install(target, destination):
    """Preserve old callers without installing an incompatible native collector."""
    executable = "samply.exe" if target.startswith("windows") else "samply"
    raise ValueError(
        "Hunin requires its pinned native collector with the lifetime extension; "
        "stock Samply releases are incompatible. Build the collector with: "
        "python collectors/native/build.py --output "
        f"collectors/native/bin/{executable} --cache /your/build-cache"
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--platform", choices=PLATFORMS, required=True)
    parser.add_argument("--destination", type=Path, required=True)
    parser.add_argument("--github-env", action="store_true")
    args = parser.parse_args()
    try:
        install(args.platform, args.destination)
    except ValueError as error:
        parser.error(str(error))


if __name__ == "__main__":
    main()
