#!/usr/bin/env python3
"""Compatibility entrypoint for native outcome acceptance."""

from acceptance.outcome import (
    execute as execute,
    verify as verify,
    main as main,
)


if __name__ == "__main__":
    main()
