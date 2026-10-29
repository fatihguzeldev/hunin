#!/usr/bin/env python3
"""Stable command entrypoint; implementation belongs to its distribution context."""

if __package__:
    from .provisioning.runtime import (
        MANIFEST as MANIFEST,
        PLATFORMS as PLATFORMS,
        asset as asset,
        digest as digest,
        install as install,
        main as main,
        tree_digest as tree_digest,
        unpack as unpack,
    )
else:
    from provisioning.runtime import (
        MANIFEST as MANIFEST,
        PLATFORMS as PLATFORMS,
        asset as asset,
        digest as digest,
        install as install,
        main as main,
        tree_digest as tree_digest,
        unpack as unpack,
    )

if __name__ == "__main__":
    main()
