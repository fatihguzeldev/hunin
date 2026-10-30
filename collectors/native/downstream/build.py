#!/usr/bin/env python3
"""Build Hunin's checksum-pinned native collector into a distributable binary."""

import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

from . import source as source_inputs, toolchain as rust_toolchain
from .checks import required_checks, run_checks


def build(output, cache, archive=None, toolchain="stable", test_filters=()):
    output = output.parent.resolve() / output.name
    if output.is_symlink():
        raise ValueError(f"Preserving existing native collector symlink: {output}")
    source, workspace, identity, patch_sha256 = source_inputs.prepare_inputs(
        cache, archive
    )
    source_sha256 = source_inputs.tree_digest(source)
    compiler, host = rust_toolchain.inspect(toolchain)
    target = workspace / "target"
    environment = dict(
        os.environ, CARGO_TARGET_DIR=str(target), HUNIN_NATIVE_PATCH_SHA256=patch_sha256
    )
    # Build the host explicitly; an inherited cross target must not change the bundle.
    subprocess.run(
        [
            "cargo",
            f"+{toolchain}",
            "build",
            "--locked",
            "--release",
            "--target",
            host,
            "-p",
            source_inputs.MANIFEST["cargo_package"],
        ],
        cwd=source,
        env=environment,
        check=True,
        timeout=1800,
    )
    source_checks = run_checks(
        source, environment, toolchain, host, required_checks(host, test_filters)
    )
    if source_inputs.tree_digest(source) != source_sha256:
        raise ValueError(
            "Native collector source changed during build or source checks"
        )
    binary = target / host / "release" / ("samply.exe" if os.name == "nt" else "samply")
    if not binary.is_file():
        raise ValueError(f"Native collector build produced no executable: {binary}")
    help_text = subprocess.check_output(
        [str(binary), "record", "--help"], text=True, timeout=10
    )
    if source_inputs.MANIFEST["extension"]["flag"] not in help_text:
        raise ValueError(
            "Built native collector does not expose the pinned lifetime extension"
        )
    version = subprocess.check_output(
        [str(binary), "--version"], text=True, timeout=10
    ).strip()
    if version != f"samply {source_inputs.MANIFEST['version']}":
        raise ValueError(
            f"Built native collector reported an unexpected version: {version}"
        )
    receipt = output.with_name(output.name + ".provenance.json")
    if output.exists():
        previous = json.loads(receipt.read_text()) if receipt.is_file() else {}
        if previous.get("binary_sha256") != source_inputs.digest(output):
            raise ValueError(
                f"Preserving unrelated or modified native collector executable: {output}"
            )
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(
        prefix=".samply-publish-", dir=output.parent, delete=False
    ) as temporary:
        staged = Path(temporary.name)
    try:
        shutil.copy2(binary, staged)
        staged.chmod(0o755)
        staged.replace(output)
    finally:
        staged.unlink(missing_ok=True)
    for name in ("LICENSE-MIT", "LICENSE-APACHE"):
        shutil.copy2(source / name, output.parent / name)
    receipt.write_text(
        json.dumps(
            {
                "schema_version": 1,
                "identity": identity,
                "version": source_inputs.MANIFEST["version"],
                "extension": source_inputs.MANIFEST["extension"],
                "patch_sha256": patch_sha256,
                "source_sha256": source_sha256,
                "source_checks": source_checks,
                "rustc": compiler,
                "target": host,
                "binary_sha256": source_inputs.digest(output),
            },
            indent=2,
        )
        + "\n"
    )
    return output


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--cache", type=Path, required=True)
    parser.add_argument(
        "--archive",
        type=Path,
        help="Reuse a local archive after verifying the pinned digest",
    )
    parser.add_argument("--rust-toolchain", default="stable")
    parser.add_argument(
        "--test-filter",
        action="append",
        default=[],
        help="Add a Samply source-test filter; required native regressions always run",
    )
    parser.add_argument("--github-env", action="store_true")
    args = parser.parse_args()
    if args.github_env and not os.environ.get("GITHUB_ENV"):
        parser.error("--github-env requires a GitHub Actions job environment")
    binary = build(
        args.output.absolute(),
        args.cache.resolve(),
        args.archive.resolve() if args.archive else None,
        args.rust_toolchain,
        args.test_filter,
    )
    if args.github_env:
        with open(os.environ["GITHUB_ENV"], "a", encoding="utf-8") as environment:
            environment.write(f"HUNIN_SAMPLY={binary}\n")
    print(binary)


if __name__ == "__main__":
    main()
