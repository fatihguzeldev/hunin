import hashlib
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from . import bundle as installer

HOST_PLATFORM = installer.host_platform


class InstalledBundleTests(unittest.TestCase):
    def setUp(self):
        # These integrity/link fixtures model Linux bundles, not executable
        # host acceptance. Host identity has separate positive/negative controls.
        host = patch.object(installer, "host_platform", return_value="linux-x64")
        host.start()
        self.addCleanup(host.stop)

    def bundle(self, root, version, target="linux-x64"):
        root.mkdir(parents=True)
        executable = "hunin.exe" if target.startswith("windows") else "hunin"
        files = {executable: b"test-binary", "ui/index.html": b"test-ui"}
        for name, data in files.items():
            path = root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(data)
        (root / "manifest.json").write_text(
            json.dumps(
                {
                    "schema_version": 1,
                    "version": version,
                    "platform": target,
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

    def test_wrong_host_fails_before_creating_an_installation(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            bundle = self.bundle(root / "bundle", "0.2.0")
            with patch.object(installer, "host_platform", return_value="macos-arm64"):
                with self.assertRaisesRegex(ValueError, "host.*macos-arm64"):
                    installer.install(bundle, root / "installed", root / "bin")
            self.assertFalse((root / "installed").exists())
            self.assertFalse((root / "bin").exists())

    def test_host_identity_recognizes_selected_platforms_and_architecture_aliases(self):
        with patch.object(installer, "host_platform", wraps=HOST_PLATFORM):
            for system, machine, expected in [
                ("Linux", "x86_64", "linux-x64"),
                ("Linux", "aarch64", "linux-arm64"),
                ("Darwin", "x86_64", "macos-x64"),
                ("Darwin", "arm64", "macos-arm64"),
                ("Windows", "AMD64", "windows-x64"),
            ]:
                with (
                    self.subTest(system=system, machine=machine),
                    patch.object(installer.platform, "system", return_value=system),
                    patch.object(installer.platform, "machine", return_value=machine),
                ):
                    self.assertEqual(installer.host_platform(), expected)
            with (
                patch.object(installer.platform, "system", return_value="Windows"),
                patch.object(installer.platform, "machine", return_value="ARM64"),
            ):
                with self.assertRaisesRegex(
                    ValueError, "Unsupported installation host"
                ):
                    installer.host_platform()

    def test_changed_staged_bytes_never_publish_an_installation(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            bundle = self.bundle(root / "bundle", "0.2.0")
            prefix = root / "installed"
            prefix.mkdir()
            unrelated = prefix / "keep.txt"
            unrelated.write_text("preserve unrelated data")
            copy = installer.shutil.copy2

            def changed_copy(source, target):
                result = copy(source, target)
                if Path(target).name == "hunin":
                    Path(target).write_bytes(b"changed during staging")
                return result

            with (
                patch.object(installer, "host_platform", return_value="linux-x64"),
                patch.object(installer.shutil, "copy2", side_effect=changed_copy),
            ):
                with self.assertRaisesRegex(ValueError, "checksum mismatch"):
                    installer.install(bundle, prefix)
            self.assertEqual(list(prefix.iterdir()), [unrelated])
            self.assertEqual(unrelated.read_text(), "preserve unrelated data")

    @unittest.skipIf(
        os.name == "nt", "Creating local symlinks requires host privileges"
    )
    def test_same_version_dangling_destination_is_preserved(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            bundle = self.bundle(root / "bundle", "0.2.0")
            prefix = root / "installed"
            prefix.mkdir()
            destination = prefix / "0.2.0-linux-x64"
            missing = root / "missing-version"
            destination.symlink_to(missing)
            with self.assertRaisesRegex(ValueError, "already exists"):
                installer.install(bundle, prefix)
            self.assertEqual(destination.readlink(), missing)

    def test_windows_selects_an_explicit_executable_path_without_bin_activation(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            bundle = self.bundle(root / "bundle", "0.2.0", "windows-x64")
            with patch.object(installer, "host_platform", return_value="windows-x64"):
                with self.assertRaisesRegex(ValueError, "Unix-only"):
                    installer.install(bundle, root / "refused", root / "bin")
                self.assertFalse((root / "refused").exists())
                installed = installer.install(bundle, root / "installed")
                self.assertEqual(installed.name, "hunin.exe")
                with self.assertRaisesRegex(ValueError, "Unix-only"):
                    installer.activate(root / "installed", "0.2.0", root / "bin")
                self.assertFalse((root / "bin").exists())

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

    @unittest.skipIf(os.name == "nt", "Activation through a bin symlink is Unix-only")
    def test_activation_switches_verified_versions_without_touching_old_bytes_or_data(
        self,
    ):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            prefix = root / "installed with spaces"
            older = self.bundle(prefix / "0.1.0-linux-x64", "0.1.0")
            newer = self.bundle(prefix / "0.2.0-linux-x64", "0.2.0")
            data = root / "recording data"
            data.write_text("not an installer-owned path")
            (root / "bin").mkdir()
            link = root / "bin/hunin"
            link.symlink_to(newer / "hunin")
            with (newer / "hunin").open("rb") as already_open:
                activation = installer.activate(prefix, "0.1.0", root / "bin")
                self.assertEqual(link.resolve(), (older / "hunin").resolve())
                self.assertEqual(already_open.read(), b"test-binary")
            self.assertEqual(activation["component_compatibility"], "legacy_unknown")
            self.assertEqual(
                Path(activation["executable"]), (older / "hunin").resolve()
            )
            self.assertEqual((newer / "hunin").read_bytes(), b"test-binary")
            self.assertEqual(data.read_text(), "not an installer-owned path")

    @unittest.skipIf(os.name == "nt", "Activation through a bin symlink is Unix-only")
    def test_corrupt_activation_target_and_unrelated_link_are_preserved(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            prefix = root / "installed"
            older = self.bundle(prefix / "0.1.0-linux-x64", "0.1.0")
            newer = self.bundle(prefix / "0.2.0-linux-x64", "0.2.0")
            (root / "bin").mkdir()
            link = root / "bin/hunin"
            link.symlink_to(newer / "hunin")
            (older / "hunin").write_bytes(b"changed")
            with self.assertRaisesRegex(ValueError, "checksum mismatch"):
                installer.activate(prefix, "0.1.0", root / "bin")
            self.assertEqual(link.resolve(), (newer / "hunin").resolve())
            unrelated = root / "another executable"
            unrelated.write_bytes(b"preserve")
            link.unlink()
            link.symlink_to(unrelated)
            with self.assertRaisesRegex(ValueError, "Preserving existing executable"):
                installer.activate(prefix, "0.2.0", root / "bin")
            self.assertEqual(link.readlink(), unrelated)
            self.assertEqual(unrelated.read_bytes(), b"preserve")

    @unittest.skipIf(os.name == "nt", "Activation through a bin symlink is Unix-only")
    def test_activation_publication_failure_cleans_only_its_owned_stage(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            prefix = root / "installed"
            self.bundle(prefix / "0.1.0-linux-x64", "0.1.0")
            newer = self.bundle(prefix / "0.2.0-linux-x64", "0.2.0")
            (root / "bin").mkdir()
            link = root / "bin/hunin"
            link.symlink_to(newer / "hunin")
            unrelated = root / "bin/.hunin-link"
            unrelated.write_text("an unrelated old staging path")
            with patch.object(
                Path, "replace", side_effect=OSError("controlled publication failure")
            ):
                with self.assertRaisesRegex(OSError, "controlled publication"):
                    installer.activate(prefix, "0.1.0", root / "bin")
            self.assertEqual(link.resolve(), (newer / "hunin").resolve())
            self.assertEqual(unrelated.read_text(), "an unrelated old staging path")
            self.assertFalse(list((root / "bin").glob(".hunin-activate-*")))
