import importlib.util
import json
from pathlib import Path
import tempfile
import unittest


def load(name):
    spec = importlib.util.spec_from_file_location(name, Path(__file__).with_name(name + ".py"))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


package = load("package")
installer = load("install")


class PackageTests(unittest.TestCase):
    def test_bundle_retains_assets_and_install_verifies_integrity(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "Cargo.toml").write_text('[package]\nversion="0.1.0"\n')
            for path in ("ui/dist/index.html", "sdk/python/hunin.py", "collectors/python/collect.py", "protocol/record.schema.json", "docs/installation.md", "distribution/install.py", "distribution/fetch_samply.py", "distribution/fetch_runtime.py", "distribution/runtime-assets.json"):
                target = root / path
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_text(path)
            binary = root / "hunin"
            binary.write_bytes(b"test-binary")
            archive = package.assemble(root, binary, root / "output", "linux-x64")
            import tarfile
            with tarfile.open(archive) as bundle:
                bundle.extractall(root / "extracted", filter="data")
            extracted = next((root / "extracted").iterdir())
            manifest = installer.verify(extracted)
            self.assertIn("collectors/python/collect.py", manifest["files"])
            installed = installer.install(extracted, root / "installed")
            self.assertEqual(installed.read_bytes(), b"test-binary")
            self.assertTrue((installed.parent / "ui/index.html").exists())
            (extracted / "hunin").write_bytes(b"modified")
            with self.assertRaisesRegex(ValueError, "checksum mismatch"):
                installer.verify(extracted)

    def test_manifest_rejects_traversal(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "bundle").mkdir()
            (root / "outside").write_text("private")
            (root / "bundle/manifest.json").write_text(json.dumps({"schema_version": 1, "version": "0.1.0", "platform": "linux-x64", "files": {"../outside": {"size": 7, "sha256": "unused"}}}))
            with self.assertRaisesRegex(ValueError, "Unsafe package path"):
                installer.verify(root / "bundle")


if __name__ == "__main__":
    unittest.main()
