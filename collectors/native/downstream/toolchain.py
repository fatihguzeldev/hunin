"""Observed Rust compiler identity and native host target for downstream work."""

import subprocess


def inspect(toolchain):
    compiler = subprocess.check_output(
        ["rustc", f"+{toolchain}", "--version", "--verbose"], text=True, timeout=5
    ).strip()
    host = next(
        (
            line.removeprefix("host: ")
            for line in compiler.splitlines()
            if line.startswith("host: ")
        ),
        None,
    )
    if not host:
        raise ValueError("Rust compiler did not report a native host target")
    return compiler, host
