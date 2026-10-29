import io
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest.mock import patch

from . import build as native


class NativeBuildTests(unittest.TestCase):
    def archive(self, root, unsafe=False):
        archive = root / "source.tar.gz"
        files = {"source/Cargo.lock": b"# locked\n", "source/value.txt": b"before\n"}
        if unsafe:
            files["../outside"] = b"unsafe"
        with tarfile.open(archive, "w:gz") as bundle:
            for name, data in files.items():
                entry = tarfile.TarInfo(name)
                entry.size = len(data)
                bundle.addfile(entry, io.BytesIO(data))
        return archive

    def test_source_archive_requires_the_exact_pinned_bytes(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            archive = self.archive(root)
            manifest = {"upstream": {"archive_sha256": native.digest(archive)}}
            with patch.object(native, "MANIFEST", manifest):
                self.assertEqual(native.source_archive(root, archive), archive)
                archive.write_bytes(b"modified")
                with self.assertRaisesRegex(ValueError, "checksum mismatch"):
                    native.source_archive(root, archive)

    def test_source_cache_applies_patches_and_rejects_modified_inputs_on_reuse(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            cache = root / "cache"
            cache.mkdir()
            archive = self.archive(root)
            change = b"diff --git a/value.txt b/value.txt\n--- a/value.txt\n+++ b/value.txt\n@@ -1 +1 @@\n-before\n+after\n"
            patches = [(Path("0001.patch"), change)]
            source, workspace, identity = native.prepare_source(cache, archive, patches)
            self.assertEqual((source / "value.txt").read_text(), "after\n")
            self.assertEqual(
                native.prepare_source(cache, archive, patches),
                (source, workspace, identity),
            )
            (source / "value.txt").write_text("modified\n")
            with self.assertRaisesRegex(ValueError, "Preserving modified"):
                native.prepare_source(cache, archive, patches)

    def test_source_archive_cannot_write_outside_its_root(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            archive = self.archive(root, unsafe=True)
            with self.assertRaisesRegex(ValueError, "Unsafe source archive path"):
                native.unpack(archive, root / "unpacked")
            self.assertFalse((root / "outside").exists())


if __name__ == "__main__":
    unittest.main()
