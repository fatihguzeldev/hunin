#!/usr/bin/env python3
"""Stable command entrypoint; implementation belongs to its distribution context."""

if __package__:
    from .verification.installed import (
        adjacent_import as adjacent_import,
        execute as execute,
        main as main,
        unused_port as unused_port,
        verify as verify,
    )
else:
    from verification.installed import (
        adjacent_import as adjacent_import,
        execute as execute,
        main as main,
        unused_port as unused_port,
        verify as verify,
    )

if __name__ == "__main__":
    main()
