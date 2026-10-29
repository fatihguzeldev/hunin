"""Python entry-point semantics shared by collection adapters."""

from pathlib import Path
import builtins
from importlib.machinery import (
    BuiltinImporter,
    SourceFileLoader,
    SourcelessFileLoader,
)
import io
import os
from pkgutil import get_importer, read_code
import runpy
import sys
from types import ModuleType


def _main_module():
    main = ModuleType("__main__")
    main.__dict__["__builtins__"] = builtins
    if sys.version_info < (3, 14):
        main.__dict__["__annotations__"] = {}
    sys.modules["__main__"] = main
    return main


def _run_script(path):
    main = _main_module()
    if get_importer(path) is not None:
        sys.path[0] = (
            path if sys.implementation.name == "graalpy" else os.path.abspath(path)
        )
        runpy._run_module_as_main("__main__", alter_argv=False)
        return
    # CPython's Windows launcher uses GetFullPathNameW rather than realpath.
    # Preserve short-name/junction spelling; POSIX script lookup resolves links.
    sys.path[0] = (
        os.path.dirname(os.path.abspath(path))
        if os.name == "nt"
        else str(Path(path).resolve().parent)
    )
    filename = path if sys.implementation.name == "graalpy" else os.path.abspath(path)
    # Delegate bytecode recognition/loading to the selected interpreter's
    # stdlib, including its native magic/version checks. Never parse pyc here.
    with io.open_code(filename) as source:
        code = read_code(source)
    if code is None:
        code = runpy._get_code_from_file(filename)
        loader_path = path if sys.implementation.name == "pypy" else filename
        loader = SourceFileLoader("__main__", loader_path)
    else:
        loader = SourcelessFileLoader("__main__", filename)
    main.__file__ = filename
    main.__cached__ = None
    if sys.implementation.name != "graalpy":
        main.__loader__ = loader
    exec(code, main.__dict__)


def run_target(target):
    if not target:
        raise SystemExit("a Python script, -m module or -c code is required")
    if target[0] == "-m":
        if len(target) < 2:
            raise SystemExit("-m requires a module name")
        sys.argv = target[1:]
        sys.path[0] = os.getcwd()
        _main_module()
        runpy._run_module_as_main(target[1], alter_argv=True)
    elif target[0] == "-c":
        if len(target) < 2:
            raise SystemExit("-c requires Python code")
        sys.argv = ["-c", *target[2:]]
        sys.path[0] = ""
        main = _main_module()
        if sys.implementation.name in ("cpython", "pypy"):
            main.__loader__ = BuiltinImporter
        exec(compile(target[1], "<string>", "exec"), main.__dict__)
    else:
        if target[0].startswith("-"):
            raise SystemExit("unsupported Python entry-point option: " + target[0])
        sys.argv = target
        _run_script(target[0])
