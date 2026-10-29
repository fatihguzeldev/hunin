#!/usr/bin/env python3
"""Stable command entrypoint; implementation belongs to its distribution context."""

if __package__:
    from .verification.native_sdk import (
        execute as execute,
        main as main,
        paged_records as paged_records,
        verify as verify,
    )
else:
    from verification.native_sdk import (
        execute as execute,
        main as main,
        paged_records as paged_records,
        verify as verify,
    )

if __name__ == "__main__":
    main()
