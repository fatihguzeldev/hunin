import hashlib
import importlib.util
import io
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest.mock import patch
import zipfile

spec = importlib.util.spec_from_file_location(
    "fetch_runtime", Path(__file__).with_name("fetch_runtime.py")
)
runtime = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runtime)


class RuntimeTests(unittest.TestCase):
    def test_manifest_has_explicit_host_coverage_and_unavailable_reason(self):
        for name, release in runtime.MANIFEST["runtimes"].items():
            self.assertEqual(
                set(release["platforms"]) | set(release.get("unavailable", {})),
                set(runtime.PLATFORMS),
            )
            for target, selected in release["platforms"].items():
                self.assertRegex(selected["sha256"], r"^[a-f0-9]{64}$")
                self.assertTrue(selected["url"].startswith("https://"))
        with self.assertRaisesRegex(ValueError, "macOS x64"):
            runtime.asset("graalpy", "macos-x64")

    def test_verified_install_reuses_pristine_tree_and_preserves_modified_directory(
        self,
    ):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            archive = root / "runtime.zip"
            with zipfile.ZipFile(archive, "w") as bundle:
                bundle.writestr("bin/example", "binary")
                bundle.writestr("lib/runtime", "dependency")
            selected = {
                "url": "https://example.invalid/runtime.zip",
                "sha256": hashlib.sha256(archive.read_bytes()).hexdigest(),
                "executable": "example",
            }
            manifest = {
                "runtimes": {
                    "test": {"version": "1", "platforms": {"linux-x64": selected}}
                }
            }
            with patch.object(runtime, "MANIFEST", manifest):
                binary = runtime.install(
                    "test", "linux-x64", root / "installed", archive
                )
                self.assertEqual(binary.read_text(), "binary")
                self.assertEqual(
                    runtime.install("test", "linux-x64", root / "installed", archive),
                    binary,
                )
                (root / "installed/lib/runtime").write_text("modified")
                with self.assertRaisesRegex(ValueError, "Preserving existing"):
                    runtime.install("test", "linux-x64", root / "installed", archive)
                archive.write_bytes(b"tampered")
                with self.assertRaisesRegex(ValueError, "checksum mismatch"):
                    runtime.install("test", "linux-x64", root / "second", archive)
                self.assertFalse((root / "second").exists())

    def test_archive_rejects_traversal_and_external_links_but_retains_internal_links(
        self,
    ):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            destination = root / "output"
            destination.mkdir()
            archive = root / "bad.zip"
            with zipfile.ZipFile(archive, "w") as bundle:
                bundle.writestr("../outside", "bad")
            with self.assertRaisesRegex(ValueError, "Unsafe runtime archive path"):
                runtime.unpack(archive, destination)
            archive = root / "links.tar.gz"
            with tarfile.open(archive, "w:gz") as bundle:
                entry = tarfile.TarInfo("bin/program")
                entry.size = 2
                bundle.addfile(entry, io.BytesIO(b"ok"))
                link = tarfile.TarInfo("bin/alias")
                link.type = tarfile.LNKTYPE
                link.linkname = "bin/program"
                bundle.addfile(link)
            runtime.unpack(archive, destination)
            self.assertEqual((destination / "bin/alias").read_text(), "ok")
            archive = root / "external.tar.gz"
            with tarfile.open(archive, "w:gz") as bundle:
                link = tarfile.TarInfo("bad")
                link.type = tarfile.SYMTYPE
                link.linkname = "../../outside"
                bundle.addfile(link)
            with self.assertRaises(tarfile.FilterError):
                runtime.unpack(archive, destination)


if __name__ == "__main__":
    unittest.main()
