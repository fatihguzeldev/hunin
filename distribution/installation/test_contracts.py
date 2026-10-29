import hashlib
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from . import bundle as installer


def product_fixture(root):
    native = {
        "version": "test-native-1",
        "extension": {"flag": "--lifetime-output", "schema_version": 1},
    }
    contents = {
        "hunin": b"test executable",
        "ui/index.html": b"test UI",
        "protocol/record.schema.json": json.dumps(
            {"$id": "https://test/record-v1"}
        ).encode(),
        "protocol/response.schema.json": json.dumps(
            {"$id": "https://test/response-v1"}
        ).encode(),
        "protocol/operations.json": b"[]",
        "collectors/native/manifest.json": json.dumps(native).encode(),
    }
    for name, data in contents.items():
        target = root / name
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(data)

    def digest(name):
        return hashlib.sha256(contents[name]).hexdigest()

    info = {
        "schema_version": 1,
        "app": {
            "name": "hunin",
            "version": "0.1.0",
            "source_revision": None,
            "target": {"os": "linux", "arch": "x86_64"},
        },
        "contracts": {
            "record": {
                "id": "https://test/record-v1",
                "sha256": digest("protocol/record.schema.json"),
            },
            "responses": {
                "id": "https://test/response-v1",
                "sha256": digest("protocol/response.schema.json"),
            },
            "operations": {"sha256": digest("protocol/operations.json")},
            "native_manifest": {
                "sha256": digest("collectors/native/manifest.json"),
                **native,
            },
        },
        "storage": {"maximum_schema_version": 2},
    }
    manifest = {
        "schema_version": 1,
        "platform": "linux-x64",
        "version": "0.1.0",
        "files": {
            name: {"size": len(data), "sha256": hashlib.sha256(data).hexdigest()}
            for name, data in contents.items()
        },
        "product_build_info": info,
    }
    (root / "manifest.json").write_text(json.dumps(manifest))
    return manifest


class ProductContractTests(unittest.TestCase):
    def test_contract_metadata_must_match_packaged_bytes_and_types(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            baseline = product_fixture(root)
            installer.verify(root)
            for path, value in [
                (("contracts", "record", "sha256"), "0" * 64),
                (("contracts", "record", "id"), "https://different/record"),
                (("contracts", "native_manifest", "version"), "different-native"),
                (("app", "version"), "9.0.0"),
                (("app", "target"), {"os": "macos", "arch": "aarch64"}),
                (("storage", "maximum_schema_version"), True),
            ]:
                with self.subTest(path=path):
                    manifest = json.loads(json.dumps(baseline))
                    target = manifest["product_build_info"]
                    for key in path[:-1]:
                        target = target[key]
                    target[path[-1]] = value
                    (root / "manifest.json").write_text(json.dumps(manifest))
                    with self.assertRaisesRegex(ValueError, "[Cc]ontract|[Pp]roduct"):
                        installer.verify(root)

    def test_staged_executable_descriptor_is_rechecked_before_publication(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / "bundle"
            source.mkdir()
            manifest = product_fixture(source)
            different = json.loads(json.dumps(manifest["product_build_info"]))
            different["storage"]["maximum_schema_version"] = 99
            with (
                patch.object(installer, "host_platform", return_value="linux-x64"),
                patch.object(
                    installer, "probe_binary", return_value=different, create=True
                ),
            ):
                with self.assertRaisesRegex(ValueError, "[Dd]escriptor"):
                    installer.install(source, root / "installed")
            self.assertFalse((root / "installed/0.1.0-linux-x64").exists())
            self.assertFalse(list((root / "installed").glob(".hunin-install-*")))

    @unittest.skipIf(os.name == "nt", "Activation through a bin symlink is Unix-only")
    def test_activation_reports_verified_contracts_without_opening_recording_data(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            directory = root / "installed/0.1.0-linux-x64"
            directory.mkdir(parents=True)
            manifest = product_fixture(directory)
            data = root / "invalid-store.sqlite"
            data.write_bytes(b"not a database")
            with (
                patch.object(installer, "host_platform", return_value="linux-x64"),
                patch.object(
                    installer,
                    "probe_binary",
                    return_value=manifest["product_build_info"],
                    create=True,
                ),
            ):
                activated = installer.activate(
                    root / "installed", "0.1.0", root / "bin"
                )
            self.assertEqual(
                activated["component_compatibility"], "declared_contracts_verified"
            )
            self.assertEqual(data.read_bytes(), b"not a database")

    def test_historical_descriptor_without_target_remains_explicitly_unknown(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            manifest = product_fixture(root)
            del manifest["product_build_info"]["app"]["target"]
            (root / "manifest.json").write_text(json.dumps(manifest))
            installer.verify(root)
            with patch.object(
                installer, "probe_binary", return_value=manifest["product_build_info"]
            ):
                self.assertEqual(
                    installer.verify_executable(root, manifest), "legacy_unknown_target"
                )
