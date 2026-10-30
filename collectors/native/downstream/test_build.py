from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from . import build as native, source as source_inputs, toolchain as rust_toolchain


class NativeBuildTests(unittest.TestCase):
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
                    source_inputs,
                    "prepare_inputs",
                    return_value=(source, workspace, identity, "patch-digest"),
                ),
                patch.object(
                    rust_toolchain,
                    "inspect",
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
                    source_inputs,
                    "prepare_inputs",
                    return_value=(source, workspace, {"patches": []}, "patch-digest"),
                ),
                patch.object(
                    rust_toolchain,
                    "inspect",
                    return_value=("selected compiler", host),
                ),
                patch.object(native.subprocess, "run"),
                patch.object(native, "run_checks", return_value=source_checks),
                patch.object(
                    native.subprocess,
                    "check_output",
                    side_effect=[
                        source_inputs.MANIFEST["extension"]["flag"],
                        f"samply {source_inputs.MANIFEST['version']}",
                    ],
                ),
            ):
                native.build(output, root / "cache")
            receipt = native.json.loads(
                output.with_name(output.name + ".provenance.json").read_text()
            )
            self.assertEqual(receipt["source_checks"], source_checks)
            self.assertEqual(
                receipt["source_sha256"], source_inputs.tree_digest(source)
            )
            self.assertEqual(receipt["rustc"], "selected compiler")


if __name__ == "__main__":
    unittest.main()
