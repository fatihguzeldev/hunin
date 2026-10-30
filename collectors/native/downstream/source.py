"""Pinned source acquisition, patch application and immutable build-input cache."""

import hashlib
import json
from pathlib import Path
import subprocess
import tarfile
import tempfile
import urllib.request

ROOT = Path(__file__).resolve().parents[1]
MANIFEST = json.loads((ROOT / "manifest.json").read_text())


def digest(path):
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def tree_digest(directory):
    result = hashlib.sha256()
    for path in sorted(directory.rglob("*")):
        if path.is_symlink():
            raise ValueError(f"Unexpected symlink in native collector source: {path}")
        if path.is_file():
            result.update(
                f"{path.relative_to(directory).as_posix()}\0{digest(path)}\n".encode()
            )
    return result.hexdigest()


def unpack(archive, destination):
    total = 0
    with tarfile.open(archive) as bundle:
        for entry in bundle.getmembers():
            path = destination / entry.name
            if (
                "\\" in entry.name
                or not path.resolve().is_relative_to(destination.resolve())
                or not (entry.isfile() or entry.isdir())
            ):
                raise ValueError(f"Unsafe source archive path: {entry.name}")
            total += entry.size
            if total > 256 * 1024**2:
                raise ValueError(
                    "Native collector source exceeds the unpacked size limit"
                )
        bundle.extractall(destination, filter="data")
    roots = list(destination.iterdir())
    if len(roots) != 1 or not (roots[0] / "Cargo.lock").is_file():
        raise ValueError("Native collector source has no unique locked workspace")
    return roots[0]


def source_archive(cache, supplied=None):
    expected = MANIFEST["upstream"]["archive_sha256"]
    archive = supplied or cache / f"{expected}.tar.gz"
    if not archive.exists():
        if supplied:
            raise ValueError(f"Source archive does not exist: {archive}")
        with tempfile.NamedTemporaryFile(
            prefix=".samply-download-", dir=cache, delete=False
        ) as output:
            temporary = Path(output.name)
        try:
            with temporary.open("wb") as output:
                request = urllib.request.Request(
                    MANIFEST["upstream"]["archive_url"],
                    headers={"User-Agent": "hunin-native-collector-build"},
                )
                with urllib.request.urlopen(request, timeout=60) as source:
                    size = 0
                    while chunk := source.read(1024 * 1024):
                        size += len(chunk)
                        if size > 64 * 1024**2:
                            raise ValueError(
                                "Native collector source exceeds the download size limit"
                            )
                        output.write(chunk)
            if digest(temporary) != expected:
                raise ValueError("Native collector source archive checksum mismatch")
            temporary.replace(archive)
        finally:
            temporary.unlink(missing_ok=True)
    if digest(archive) != expected:
        raise ValueError("Native collector source archive checksum mismatch")
    return archive


def prepare_source(cache, archive, patches):
    identity = {
        "upstream": MANIFEST["upstream"],
        "patches": [
            {"name": path.name, "sha256": hashlib.sha256(data).hexdigest()}
            for path, data in patches
        ],
    }
    key = hashlib.sha256(json.dumps(identity, sort_keys=True).encode()).hexdigest()
    workspace = cache / key
    receipt = workspace / "source.json"
    if workspace.exists():
        saved = json.loads(receipt.read_text()) if receipt.is_file() else {}
        source = workspace / "source"
        if (
            saved.get("identity") != identity
            or not source.is_dir()
            or tree_digest(source) != saved.get("source_sha256")
        ):
            raise ValueError(
                f"Preserving modified native collector build cache: {workspace}"
            )
        return source, workspace, identity
    with tempfile.TemporaryDirectory(prefix=".samply-source-", dir=cache) as temporary:
        staged = Path(temporary) / "workspace"
        staged.mkdir()
        unpacked = unpack(archive, staged / "unpacked")
        source = staged / "source"
        unpacked.rename(source)
        (staged / "unpacked").rmdir()
        for index, (path, data) in enumerate(patches):
            normalized = Path(temporary) / f"{index:04d}-{path.name}"
            normalized.write_bytes(data)
            subprocess.run(
                ["git", "apply", "--check", str(normalized)],
                cwd=source,
                check=True,
                timeout=60,
            )
            subprocess.run(
                ["git", "apply", str(normalized)], cwd=source, check=True, timeout=60
            )
        (staged / "source.json").write_text(
            json.dumps(
                {"identity": identity, "source_sha256": tree_digest(source)}, indent=2
            )
            + "\n"
        )
        staged.rename(workspace)
    return workspace / "source", workspace, identity


def prepare_inputs(cache, archive=None):
    cache.mkdir(parents=True, exist_ok=True)
    patches = [
        (path, path.read_bytes().replace(b"\r\n", b"\n"))
        for path in sorted(ROOT.glob(MANIFEST["patches"]))
    ]
    if not patches:
        raise ValueError("Native collector extension patches are missing")
    archive = source_archive(cache, archive)
    source, workspace, identity = prepare_source(cache, archive, patches)
    patch_sha256 = hashlib.sha256(
        json.dumps(identity["patches"], sort_keys=True, separators=(",", ":")).encode()
    ).hexdigest()
    return source, workspace, identity, patch_sha256
