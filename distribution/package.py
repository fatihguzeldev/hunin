#!/usr/bin/env python3
"""Stable command entrypoint; implementation belongs to its distribution context."""

from pathlib import Path

if __package__:
    from .bundle.assembly import (
        PLATFORMS as PLATFORMS,
        RUST_TARGETS as RUST_TARGETS,
        assemble as assemble,
        contents as contents,
        main as main,
        sha256 as sha256,
        verify_native_collector as verify_native_collector,
    )
else:
    from bundle.assembly import (
        PLATFORMS as PLATFORMS,
        RUST_TARGETS as RUST_TARGETS,
        assemble as assemble,
        contents as contents,
        main as main,
        sha256 as sha256,
        verify_native_collector as verify_native_collector,
    )


if __package__:
    from .bundle.metadata import build_metadata as build_metadata
    from .bundle.metadata import command_output as command_output
else:
    from bundle.metadata import build_metadata as build_metadata
    from bundle.metadata import command_output as command_output

if __name__ == "__main__":
    main(Path(__file__).resolve().parents[1])
