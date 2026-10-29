#!/usr/bin/env python3
"""Compatibility entrypoint for the pinned native downstream build."""

from downstream.build import (
    MANIFEST as MANIFEST,
    ROOT as ROOT,
    build as build,
    digest as digest,
    main as main,
    prepare_source as prepare_source,
    source_archive as source_archive,
    tree_digest as tree_digest,
    unpack as unpack,
)


if __name__ == "__main__":
    main()
