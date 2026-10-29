"""Check owned source without rewriting product code or captured evidence.

Install the pinned Python tools with `python -m pip install -r quality/requirements.txt`.
Other groups use the repository's npm dependencies, Rust toolchain, Go or Clang.
A missing tool is an error; one unavailable group does not prevent checking another.
"""

import argparse
import json
from pathlib import Path
import subprocess
import sys


ROOT = Path(__file__).resolve().parent.parent
SUFFIXES = {
    "js": {".ts", ".mts"},
    "python": {".py"},
    "rust": {".rs"},
    "go": {".go"},
    "native": {".c", ".cpp", ".h", ".hpp"},
}
# These sources are embedded in retained profiles/maps or asserted source controls.
# New source fixtures should be classified by their owner before automatic rewriting.
EVIDENCE_SOURCES = (
    "src/importers/fixtures/",
    "src/collection/fixtures/",
    "collectors/python/fixtures/",
    "collectors/native/fixtures/",
    "tests/workloads/",
)


def inventory(group):
    output = subprocess.check_output(
        ["git", "ls-files", "-z", "--cached", "--others", "--exclude-standard"],
        cwd=ROOT,
    )
    files = sorted(set(output.decode().rstrip("\0").split("\0")))
    return [
        name
        for name in files
        if Path(name).suffix in SUFFIXES[group]
        and not name.startswith(EVIDENCE_SOURCES)
        and not name.endswith(".generated.ts")
    ]


def run(command, cwd=ROOT, *, capture=False):
    print(f"[{Path(cwd).relative_to(ROOT) or '.'}] {' '.join(command)}", flush=True)
    try:
        return subprocess.run(
            command, cwd=cwd, text=True, stdout=subprocess.PIPE if capture else None
        )
    except FileNotFoundError:
        raise RuntimeError(f"Required command is unavailable: {command[0]}") from None


def check(group):
    files = inventory(group)
    if group == "js":
        npm = "npm.cmd" if sys.platform == "win32" else "npm"
        return [
            run([npm, "run", "lint:js"]).returncode,
            run([npm, "run", "format:js"]).returncode,
            run([npm, "run", "check:runtime"]).returncode,
            run([npm, "run", "typecheck:ui"]).returncode,
        ]
    if group == "python":
        return [
            run(
                [
                    sys.executable,
                    "-m",
                    "ruff",
                    "check",
                    "--output-format",
                    "concise",
                    *files,
                ]
            ).returncode,
            run(
                [
                    sys.executable,
                    "-m",
                    "ruff",
                    "format",
                    "--check",
                    "--output-format",
                    "concise",
                    *files,
                ]
            ).returncode,
        ]
    if group == "rust":
        return [
            run(["cargo", "fmt", "--all", "--", "--check"]).returncode,
            run(
                [
                    "cargo",
                    "clippy",
                    "--workspace",
                    "--all-targets",
                    "--locked",
                    "--",
                    "-D",
                    "warnings",
                ]
            ).returncode,
        ]
    if group == "go":
        formatted = run(["gofmt", "-l", *files], capture=True)
        print(formatted.stdout, end="")
        results = [formatted.returncode or bool(formatted.stdout.strip())]
        for module in ("sdk/go", "collectors/go/fixtures"):
            results.append(run(["go", "vet", "./..."], ROOT / module).returncode)
        results.append(
            run(["go", "vet", "../conformance/go.go"], ROOT / "sdk/go").returncode
        )
        return results
    version = run(["clang-format", "--version"], capture=True)
    if version.returncode or "version 21.1.8" not in version.stdout:
        raise RuntimeError("Install clang-format==21.1.8 from quality/requirements.txt")
    results = [run(["clang-format", "--dry-run", "--Werror", *files]).returncode]
    include = ["-Isdk/c/include", "-Isdk/cpp/include"]
    warnings = ["-Wall", "-Wextra", "-Wpedantic", "-Werror", "-fsyntax-only"]
    results.append(
        run(
            [
                "clang",
                "-std=c11",
                *warnings,
                *include,
                "sdk/c/src/hunin.c",
                "sdk/conformance/c.c",
            ]
        ).returncode
    )
    results.append(
        run(
            [
                "clang++",
                "-std=c++17",
                *warnings,
                *include,
                "sdk/conformance/cpp.cpp",
                "sdk/c/tests/concurrency.cpp",
            ]
        ).returncode
    )
    return results


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("group", choices=[*SUFFIXES, "all"])
    parser.add_argument(
        "--list", action="store_true", help="Print the owned source inventory"
    )
    args = parser.parse_args()
    groups = list(SUFFIXES) if args.group == "all" else [args.group]
    if args.list:
        print(json.dumps({group: inventory(group) for group in groups}, indent=2))
        return 0
    status = 0
    for group in groups:
        print(f"\nChecking {group}", flush=True)
        try:
            status |= int(any(check(group)))
        except RuntimeError as error:
            print(str(error), file=sys.stderr)
            status |= 2
    return status


if __name__ == "__main__":
    sys.exit(main())
