import importlib.util
import hashlib
import json
import os
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
    def bundle(self, root, version):
        root.mkdir(parents=True)
        files = {"hunin": b"test-binary", "ui/index.html": b"test-ui"}
        for name, data in files.items():
            path = root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(data)
        (root / "manifest.json").write_text(json.dumps({
            "schema_version": 1, "version": version, "platform": "linux-x64",
            "files": {name: {"size": len(data), "sha256": hashlib.sha256(data).hexdigest()} for name, data in files.items()},
        }))
        return root

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

    @unittest.skipIf(os.name == "nt", "The optional bin symlink is Unix-only")
    def test_bin_link_preserves_unrelated_and_dangling_targets(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            bundle = self.bundle(root / "bundle", "0.2.0")
            for kind in ("unrelated", "dangling"):
                with self.subTest(kind=kind):
                    case = root / kind
                    (case / "bin").mkdir(parents=True)
                    target = case / "another-program"
                    if kind == "unrelated":
                        target.write_bytes(b"unrelated executable")
                    link = case / "bin/hunin"
                    link.symlink_to(target)
                    with self.assertRaisesRegex(ValueError, "Preserving existing executable"):
                        installer.install(bundle, case / "installed", case / "bin")
                    self.assertEqual(link.readlink(), target)
                    if kind == "unrelated":
                        self.assertEqual(target.read_bytes(), b"unrelated executable")

    @unittest.skipIf(os.name == "nt", "The optional bin symlink is Unix-only")
    def test_bin_link_upgrades_only_a_verified_installation_under_its_prefix(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            prefix = root / "installed"
            previous = self.bundle(prefix / "0.1.0-linux-x64", "0.1.0")
            bundle = self.bundle(root / "bundle", "0.2.0")
            (root / "bin").mkdir()
            link = root / "bin/hunin"
            link.symlink_to(previous / "hunin")
            executable = installer.install(bundle, prefix, root / "bin")
            self.assertEqual(link.resolve(), executable.resolve())
            self.assertTrue((previous / "hunin").is_file())
            # A valid-looking directory name does not establish ownership if its bytes changed.
            (previous / "hunin").write_bytes(b"modified")
            link.unlink()
            link.symlink_to(previous / "hunin")
            self.assertFalse(installer.owned_link(link, prefix))


if __name__ == "__main__":
    unittest.main()
