#!/usr/bin/env python3
"""Compatibility entrypoint for native windows sessions acceptance."""

from acceptance.windows_sessions import (
    execute as execute,
    verify as verify,
    STOP_RESULTS as STOP_RESULTS,
    verify_direct_recording as verify_direct_recording,
    verify_direct_cleanup as verify_direct_cleanup,
    main as main,
)


if __name__ == "__main__":
    main()
