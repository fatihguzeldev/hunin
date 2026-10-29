"""Select runtime files from owned inventories and the UI build dependency graph."""

import json
from pathlib import Path, PurePosixPath


ASSETS = Path(__file__).with_name("assets.json")


def relative(name):
    if (
        not isinstance(name, str)
        or not name
        or "\\" in name
        or PurePosixPath(name).is_absolute()
        or any(part in ("", ".", "..") for part in name.split("/"))
    ):
        raise ValueError(f"Unsafe runtime inventory path: {name!r}")
    return name


def joined(prefix, name):
    return relative(name) if prefix == "." else relative(prefix) + "/" + relative(name)


def read_asset(root, name, component):
    path = root
    for part in relative(name).split("/"):
        path /= part
        if path.is_symlink():
            raise ValueError(f"Bundle source must not contain symlinks: {path}")
    if not path.is_file():
        action = (
            f"Build it first: {component['build']}"
            if component.get("build")
            else "Restore the required component source before packaging."
        )
        raise ValueError(
            f"Required runtime asset missing: {name} ({component['name']}). {action}"
        )
    return path.read_bytes()


def ui_outputs(root, component):
    name = joined(component["source_root"], component["vite_manifest"])
    manifest = json.loads(read_asset(root, name, component))
    pending = [component["entry"]]
    visited = set()
    outputs = set()
    while pending:
        key = pending.pop()
        if key in visited:
            continue
        visited.add(key)
        chunk = manifest.get(key)
        if not isinstance(chunk, dict) or not isinstance(chunk.get("file"), str):
            raise ValueError(
                f"UI build manifest is missing chunk {key!r}; {component['build']}"
            )
        outputs.add(relative(chunk["file"]))
        for kind in ("css", "assets"):
            outputs.update(relative(path) for path in chunk.get(kind, []))
        for kind in ("imports", "dynamicImports"):
            pending.extend(chunk.get(kind, []))
    return sorted(outputs)


def contents(root, target):
    inventory = json.loads(ASSETS.read_text())
    if inventory.get("schema_version") != 1:
        raise ValueError("Unsupported runtime asset inventory")
    executable = "samply.exe" if target.startswith("windows") else "samply"
    files = {}
    for component in inventory["components"]:
        entries = dict(component["files"])
        if component.get("vite_manifest"):
            entries.update({name: name for name in ui_outputs(root, component)})
        for destination, source in entries.items():
            source = source.replace("{native_executable}", executable)
            destination = destination.replace("{native_executable}", executable)
            name = joined(component["destination_root"], destination)
            if name in files:
                raise ValueError(f"Duplicate runtime asset destination: {name}")
            files[name] = read_asset(
                root, joined(component["source_root"], source), component
            )
    return files
