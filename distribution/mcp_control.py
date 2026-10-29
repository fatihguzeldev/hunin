#!/usr/bin/env python3
"""Stable command entrypoint; implementation belongs to its distribution context."""

if __package__:
    from .verification.mcp import (
        Session as Session,
        terminal as terminal,
        verify_mcp as verify_mcp,
    )
else:
    from verification.mcp import (
        Session as Session,
        terminal as terminal,
        verify_mcp as verify_mcp,
    )
