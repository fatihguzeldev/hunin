#!/usr/bin/env python3
"""Verify an extracted bundle, then install it into a user-owned directory."""

import argparse
import json
import os
from pathlib import Path

from hunin_install import activation, contracts


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
            selected = activation.activate(prefix, args.activate, bin_dir)
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
            executable = activation.install(
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
            status = contracts.component_status(installed_manifest)
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
