import sys

from .linux import LinuxClock as LinuxClock
from .macos import DarwinClock as DarwinClock
from .windows import WindowsClock as WindowsClock


def clock():
    if sys.platform == "win32":
        return WindowsClock()
    if sys.platform == "darwin":
        return DarwinClock()
    if sys.platform.startswith("linux"):
        return LinuxClock()
    raise RuntimeError(f"OS thread CPU counters unavailable on {sys.platform}")
