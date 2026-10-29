"""Stable entrypoint for the Python capture component."""

from hunin_capture.cpu_progress.clocks import (
    WindowsClock as WindowsClock,
    DarwinClock as DarwinClock,
    LinuxClock as LinuxClock,
    clock as clock,
)
