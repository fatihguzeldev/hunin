#!/usr/bin/env python3
"""Stable command entrypoint; implementation belongs to its distribution context."""

from pathlib import Path

if __package__:
    from .installation.bundle import (
        activate as activate,
        install as install,
        main as main,
        owned_link as owned_link,
        verify as verify,
    )
else:
    from installation.bundle import (
        activate as activate,
        install as install,
        main as main,
        owned_link as owned_link,
        verify as verify,
    )

if __name__ == "__main__":
    main(Path(__file__).resolve().parent)
