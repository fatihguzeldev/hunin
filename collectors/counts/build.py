"""Compile the bounded callback against the selected interpreter, never globally."""

import hashlib
import json
import os
from pathlib import Path
import shlex
import shutil
import subprocess
import sys
import sysconfig
import tempfile


ROOT = Path(__file__).resolve().parent


def file_digest(path):
    with open(path, "rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def prerequisites():
    if sys.implementation.name != "cpython" or sys.version_info < (3, 12):
        raise RuntimeError("automatic Python starts/resumes require CPython 3.12+")
    if sysconfig.get_config_var("Py_GIL_DISABLED"):
        raise RuntimeError("free-threaded CPython is not a verified count collector")
    include = Path(sysconfig.get_path("include"))
    if not (include / "Python.h").is_file():
        raise RuntimeError("matching CPython development headers are required")
    compiler = (
        ["cl"]
        if os.name == "nt"
        else shlex.split(sysconfig.get_config_var("CC") or "cc")
    )
    found = shutil.which(compiler[0])
    if not found:
        raise RuntimeError(
            "CPython count callback compiler unavailable: " + compiler[0]
        )
    compiler[0] = found
    identity = {
        "runtime": sys.version,
        "executable": str(Path(sys.executable).resolve()),
        "executable_sha256": file_digest(sys.executable),
        "include": str(include),
        "suffix": sysconfig.get_config_var("EXT_SUFFIX"),
        "compiler": compiler,
        "compiler_sha256": file_digest(found),
        "python_header_sha256": file_digest(include / "Python.h"),
        "source_sha256": hashlib.sha256(
            (ROOT / "nativecounter.c").read_bytes()
        ).hexdigest(),
    }
    return identity


def build(cache):
    identity = prerequisites()
    key = hashlib.sha256(json.dumps(identity, sort_keys=True).encode()).hexdigest()
    destination = cache / key
    name = "_hunin_counts" + identity["suffix"]
    library = destination / name
    receipt = destination / "identity.json"
    if library.is_file() and receipt.is_file():
        prior = json.loads(receipt.read_text())
        if (
            prior.get("identity") != identity
            or prior.get("sha256") != hashlib.sha256(library.read_bytes()).hexdigest()
        ):
            raise RuntimeError(
                "cached count callback identity mismatch; preserve it and choose another cache"
            )
        return {"extension": str(library), **prior}
    if destination.exists():
        raise RuntimeError(
            "incomplete callback cache entry is preserved; choose another cache"
        )
    cache.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=".counts-build-", dir=cache) as temporary:
        staging = Path(temporary)
        output = staging / name
        if os.name == "nt":
            version = f"{sys.version_info.major}{sys.version_info.minor}"
            command = [
                *identity["compiler"],
                "/nologo",
                "/LD",
                "/O2",
                "/I" + identity["include"],
                str(ROOT / "nativecounter.c"),
                "/link",
                "/LIBPATH:" + str(Path(sys.base_prefix) / "libs"),
                f"python{version}.lib",
                "/OUT:" + str(output),
            ]
        else:
            command = [
                *identity["compiler"],
                "-O3",
                "-shared",
                "-fPIC",
                "-I" + identity["include"],
                str(ROOT / "nativecounter.c"),
                "-o",
                str(output),
            ]
            if sys.platform == "darwin":
                command += ["-undefined", "dynamic_lookup"]
                sdk = subprocess.run(
                    ["xcrun", "--show-sdk-path"],
                    capture_output=True,
                    text=True,
                    timeout=5,
                )
                if sdk.returncode == 0:
                    command += ["-isysroot", sdk.stdout.strip()]
        result = subprocess.run(
            command, cwd=staging, capture_output=True, text=True, timeout=30
        )
        if result.returncode:
            raise RuntimeError(
                "CPython count callback compilation failed: " + result.stderr[-8192:]
            )
        result = {
            "identity": identity,
            "sha256": hashlib.sha256(output.read_bytes()).hexdigest(),
        }
        (staging / "identity.json").write_text(json.dumps(result, sort_keys=True))
        try:
            staging.rename(destination)
        except FileExistsError:
            return build(cache)
    return {"extension": str(library), **result}


if __name__ == "__main__":
    try:
        print(
            json.dumps(
                prerequisites()
                if sys.argv[1] == "--probe"
                else build(Path(sys.argv[1]).resolve())
            )
        )
    except Exception as error:
        print(str(error), file=sys.stderr)
        raise SystemExit(1) from None
