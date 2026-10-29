import hashlib
import json
import os
from pathlib import Path
import tempfile
import unittest

from . import bundle as installer


class InstalledBundleTests(unittest.TestCase):
    def bundle(self, root, version):
        root.mkdir(parents=True)
        files = {"hunin": b"test-binary", "ui/index.html": b"test-ui"}
        for name, data in files.items():
            path = root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(data)
        (root / "manifest.json").write_text(
            json.dumps(
                {
                    "schema_version": 1,
                    "version": version,
                    "platform": "linux-x64",
                    "files": {
                        name: {
                            "size": len(data),
                            "sha256": hashlib.sha256(data).hexdigest(),
                        }
                        for name, data in files.items()
                    },
                }
            )
        )
        return root

    def test_manifest_rejects_traversal(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "bundle").mkdir()
            (root / "outside").write_text("private")
            (root / "bundle/manifest.json").write_text(
                json.dumps(
                    {
                        "schema_version": 1,
                        "version": "0.1.0",
                        "platform": "linux-x64",
                        "files": {"../outside": {"size": 7, "sha256": "unused"}},
                    }
                )
            )
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
                    with self.assertRaisesRegex(
                        ValueError, "Preserving existing executable"
                    ):
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
