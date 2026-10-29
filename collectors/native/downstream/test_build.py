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

    def test_failed_required_source_check_preserves_the_existing_collector(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            output = root / "existing-samply"
            output.write_bytes(b"previous accepted collector")
            source = root / "source"
            workspace = root / "workspace"
            identity = {"patches": []}
            with (
                patch.object(
                    native,
                    "prepare_inputs",
                    return_value=(source, workspace, identity, "patch-digest"),
                ),
                patch.object(
                    native,
                    "inspect_toolchain",
                    return_value=("selected compiler", "x86_64-pc-windows-msvc"),
                ),
                patch.object(native.subprocess, "run"),
                patch.object(
                    native,
                    "run_checks",
                    side_effect=ValueError("required guard failed"),
                ),
            ):
                with self.assertRaisesRegex(ValueError, "required guard failed"):
                    native.build(output, root / "cache")
            self.assertEqual(output.read_bytes(), b"previous accepted collector")
            self.assertFalse(output.with_suffix(".provenance.json").exists())

    def test_receipt_retains_source_guard_results_and_the_bound_source_digest(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / "source"
            source.mkdir()
            for name in ("LICENSE-MIT", "LICENSE-APACHE"):
                (source / name).write_text("controlled license")
            workspace = root / "workspace"
            host = "controlled-host"
            binary = (
                workspace
                / "target"
                / host
                / "release"
                / ("samply.exe" if native.os.name == "nt" else "samply")
            )
            binary.parent.mkdir(parents=True)
            binary.write_bytes(b"controlled collector bytes")
            source_checks = [{"package": "samply-symbols", "passed": 1}]
            output = root / "published" / binary.name
            with (
                patch.object(
                    native,
                    "prepare_inputs",
                    return_value=(source, workspace, {"patches": []}, "patch-digest"),
                ),
                patch.object(
                    native,
                    "inspect_toolchain",
                    return_value=("selected compiler", host),
                ),
                patch.object(native.subprocess, "run"),
                patch.object(native, "run_checks", return_value=source_checks),
                patch.object(
                    native.subprocess,
                    "check_output",
                    side_effect=[
                        native.MANIFEST["extension"]["flag"],
                        f"samply {native.MANIFEST['version']}",
                    ],
                ),
            ):
                native.build(output, root / "cache")
            receipt = native.json.loads(
                output.with_name(output.name + ".provenance.json").read_text()
            )
            self.assertEqual(receipt["source_checks"], source_checks)
            self.assertEqual(receipt["source_sha256"], native.tree_digest(source))
            self.assertEqual(receipt["rustc"], "selected compiler")


if __name__ == "__main__":
    unittest.main()
