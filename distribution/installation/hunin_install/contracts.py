"""Pure validation of executable product contracts against selected bundle bytes."""

import json

CONTRACT_FILES = {
    "record": "protocol/record.schema.json",
    "responses": "protocol/response.schema.json",
    "operations": "protocol/operations.json",
    "native_manifest": "collectors/native/manifest.json",
    "runtime_inventory": "bundle-assets.json",
}


def compiled_platform(target):
    platforms = {
        ("linux", "x86_64"): "linux-x64",
        ("linux", "aarch64"): "linux-arm64",
        ("macos", "x86_64"): "macos-x64",
        ("macos", "aarch64"): "macos-arm64",
        ("windows", "x86_64"): "windows-x64",
    }
    if not isinstance(target, dict):
        raise ValueError("Invalid product compiled target")
    platform_name = platforms.get((target.get("os"), target.get("arch")))
    if platform_name is None:
        raise ValueError("Unsupported product compiled target")
    return platform_name


def check_product(
    info,
    version,
    files,
    read,
    target=None,
    require_target=False,
    require_runtime_inventory=False,
):
    """Validate declared executable contracts against already-hashed files."""
    try:
        if (
            not isinstance(info, dict)
            or type(info["schema_version"]) is not int
            or info["schema_version"] != 1
        ):
            raise ValueError("Unsupported product descriptor schema")
        app = info["app"]
        if "target" in app:
            if target is not None and compiled_platform(app["target"]) != target:
                raise ValueError(
                    "Product compiled target does not match package platform"
                )
        elif require_target:
            raise ValueError(
                "Product descriptor has no compiled target; rebuild the binary before packaging"
            )
        if (
            app["name"] != "hunin"
            or app["version"] != version
            or not (
                app["source_revision"] is None
                or isinstance(app["source_revision"], str)
            )
        ):
            raise ValueError("Product identity does not match package version")
        maximum = info["storage"]["maximum_schema_version"]
        if type(maximum) is not int or maximum < 0:
            raise ValueError("Invalid product storage contract")
        for name, path in CONTRACT_FILES.items():
            if name == "runtime_inventory" and name not in info["contracts"]:
                if require_runtime_inventory:
                    raise ValueError(
                        "Product has no runtime inventory binding; rebuild the binary before packaging"
                    )
                # Historical installed versions keep their declared contracts.
                continue
            contract = info["contracts"][name]
            if contract["sha256"] != files[path]["sha256"]:
                raise ValueError(
                    f"Product contract hash mismatch: {path}. Rebuild the binary and its matching assets."
                )
            if name in ("record", "responses") and (
                not isinstance(contract["id"], str)
                or not contract["id"]
                or contract["id"] != json.loads(read(path))["$id"]
            ):
                raise ValueError(f"Product contract schema identity mismatch: {path}")
        native = json.loads(read(CONTRACT_FILES["native_manifest"]))
        declared = info["contracts"]["native_manifest"]
        extension = declared["extension"]
        if (
            type(extension["schema_version"]) is not int
            or not isinstance(extension["flag"], str)
            or declared["version"] != native["version"]
            or json.dumps(extension, sort_keys=True)
            != json.dumps(native["extension"], sort_keys=True)
        ):
            raise ValueError("Product native extension contract mismatch")
    except (KeyError, TypeError, AttributeError, ValueError) as error:
        raise ValueError(f"Invalid product contract descriptor: {error}") from error


def component_status(manifest):
    if "product_build_info" not in manifest:
        return "legacy_unknown"
    if "target" not in manifest["product_build_info"]["app"]:
        return "legacy_unknown_target"
    return "declared_contracts_verified"
