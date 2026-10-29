"""Python entry-point semantics shared by collection adapters."""

from pathlib import Path
import os
import runpy
import sys


def run_target(target):
    if not target:
        raise SystemExit("a Python script, -m module or -c code is required")
    if target[0] == "-m":
        if len(target) < 2:
            raise SystemExit("-m requires a module name")
        sys.argv = target[1:]
        sys.path[0] = os.getcwd()
        runpy.run_module(target[1], run_name="__main__", alter_sys=True)
    elif target[0] == "-c":
        if len(target) < 2:
            raise SystemExit("-c requires Python code")
        sys.argv = ["-c", *target[2:]]
        sys.path[0] = os.getcwd()
        exec(compile(target[1], "<string>", "exec"), {"__name__": "__main__"})
    else:
        if target[0].startswith("-"):
            raise SystemExit("unsupported Python entry-point option: " + target[0])
        sys.argv = target
        sys.path[0] = str(Path(target[0]).resolve().parent)
        runpy.run_path(target[0], run_name="__main__")
