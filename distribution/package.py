#!/usr/bin/env python3
"""Assemble a host-built Hunin bundle; packaging does not certify collector support."""
import argparse
import gzip
import hashlib
import io
import json
from pathlib import Path
import platform
import tarfile
import tomllib
import zipfile

PLATFORMS = ("linux-x64", "linux-arm64", "macos-x64", "macos-arm64", "windows-x64")
SKIP = {"node_modules", "target", "__pycache__", ".venv", ".git", ".DS_Store", ".pytest_cache"}


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def contents(root, binary):
    files = {"hunin.exe" if binary.suffix == ".exe" else "hunin": binary.read_bytes()}
    for source, destination in ((root / "ui/dist", "ui"), (root / "sdk", "sdk"),
                                (root / "collectors", "collectors"), (root / "protocol", "protocol"),
                                (root / "docs", "docs")):
        if not source.is_dir():
            raise ValueError(f"Required bundle component missing: {source}")
        for path in sorted(source.rglob("*")):
            if SKIP.intersection(path.relative_to(source).parts):
                continue
            if path.is_symlink():
                raise ValueError(f"Bundle source must not contain symlinks: {path}")
            if path.is_file():
                files[f"{destination}/{path.relative_to(source).as_posix()}"] = path.read_bytes()
    if "ui/index.html" not in files:
        raise ValueError("Build the UI before packaging (cd ui && npm ci && npm run build)")
    files["install.py"] = (root / "distribution/install.py").read_bytes()
    files["fetch_samply.py"] = (root / "distribution/fetch_samply.py").read_bytes()
    for name in ("fetch_runtime.py", "runtime-assets.json"):
        files[name] = (root / "distribution" / name).read_bytes()
    return files


def assemble(root, binary, output, target):
    version = tomllib.loads((root / "Cargo.toml").read_text())["package"]["version"]
    if (target.startswith("windows")) != (binary.suffix == ".exe"):
        raise ValueError("Windows packages require a .exe; Unix packages require a native binary")
    files = contents(root, binary)
    manifest = {"schema_version": 1, "version": version, "platform": target,
                "built_on": {"os": platform.system(), "arch": platform.machine()},
                "files": {name: {"sha256": sha256(data), "size": len(data)} for name, data in sorted(files.items())}}
    files["manifest.json"] = (json.dumps(manifest, indent=2) + "\n").encode()
    name = f"hunin-{version}-{target}"
    output.mkdir(parents=True, exist_ok=True)
    archive = output / f"{name}{'.zip' if target.startswith('windows') else '.tar.gz'}"
    if target.startswith("windows"):
        with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_DEFLATED) as bundle:
            for path, data in sorted(files.items()):
                entry = zipfile.ZipInfo(f"{name}/{path}", date_time=(1980, 1, 1, 0, 0, 0))
                entry.external_attr = 0o644 << 16
                bundle.writestr(entry, data, compress_type=zipfile.ZIP_DEFLATED)
    else:
        with archive.open("wb") as raw, gzip.GzipFile(fileobj=raw, mode="wb", mtime=0, filename="") as compressed, tarfile.open(fileobj=compressed, mode="w") as bundle:
            for path, data in sorted(files.items()):
                entry = tarfile.TarInfo(f"{name}/{path}")
                entry.size = len(data)
                entry.mode = 0o755 if path in ("hunin", "install.py") else 0o644
                entry.mtime = 0
                bundle.addfile(entry, io.BytesIO(data))
    checksum = sha256(archive.read_bytes())
    archive.with_name(archive.name + ".sha256").write_text(f"{checksum}  {archive.name}\n")
    return archive


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--platform", choices=PLATFORMS, required=True)
    parser.add_argument("--output", type=Path, default=Path("dist"))
    args = parser.parse_args()
    print(assemble(Path(__file__).resolve().parents[1], args.binary.resolve(), args.output.resolve(), args.platform))
