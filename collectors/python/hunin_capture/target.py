"""Python entry-point semantics shared by collection adapters."""

from pathlib import Path
import builtins
from importlib.machinery import BuiltinImporter
import os
import runpy
import sys
from types import ModuleType


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
        main = ModuleType("__main__")
        main.__dict__["__builtins__"] = builtins
        if sys.version_info < (3, 14):
            main.__dict__["__annotations__"] = {}
        if sys.implementation.name in ("cpython", "pypy"):
            main.__loader__ = BuiltinImporter
        # The target is the process's main module, including during exit hooks.
        # An anonymous exec namespace leaves imports pointing at the collector.
        sys.modules["__main__"] = main
        exec(compile(target[1], "<string>", "exec"), main.__dict__)
    else:
        if target[0].startswith("-"):
            raise SystemExit("unsupported Python entry-point option: " + target[0])
        sys.argv = target
        sys.path[0] = str(Path(target[0]).resolve().parent)
        runpy.run_path(target[0], run_name="__main__")
