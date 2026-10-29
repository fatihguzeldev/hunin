import importlib.util
import json
import os
from pathlib import Path
import py_compile
import subprocess
import sys
import tempfile
import types
import unittest
from unittest.mock import patch


spec = importlib.util.spec_from_file_location(
    "offline_decoder", Path(__file__).with_name("decode.py")
)
decoder = importlib.util.module_from_spec(spec)
spec.loader.exec_module(decoder)


class DecoderProvenanceTest(unittest.TestCase):
    def receipt(self, directory):
        root = Path(directory)
        package = types.ModuleType("vmprof")
        package.__path__ = [str(root)]
        reader = types.ModuleType("vmprof.reader")
        reader.__file__ = str(root / "reader.py")
        package.reader = reader
        with (
            patch.dict("sys.modules", {"vmprof": package, "vmprof.reader": reader}),
            patch.object(decoder.importlib.metadata, "version", return_value="0.6"),
            patch.object(
                decoder.importlib.util,
                "find_spec",
                side_effect=lambda name: (
                    types.SimpleNamespace(submodule_search_locations=[str(root)])
                    if name == "vmprof"
                    else None
                ),
            ),
        ):
            return decoder.provenance()

    def test_relocation_preserves_identity_but_reader_changes_do_not(self):
        with (
            tempfile.TemporaryDirectory() as first,
            tempfile.TemporaryDirectory() as second,
        ):
            for directory in (first, second):
                (Path(directory) / "reader.py").write_text("control = 1\n")
            original = self.receipt(first)
            moved = self.receipt(second)
            self.assertEqual(original["identity"], moved["identity"])
            self.assertNotEqual(
                original["evidence"]["package_root"], moved["evidence"]["package_root"]
            )
            (Path(second) / "reader.py").write_text("control = 2\n")
            self.assertNotEqual(original["identity"], self.receipt(second)["identity"])

    def test_dependency_change_during_decode_cannot_return_a_receipt(self):
        with (
            patch.object(
                decoder,
                "provenance",
                side_effect=[{"identity": "before"}, {"identity": "after"}],
            ),
            patch.object(decoder, "decode", return_value={"samples": []}),
        ):
            with self.assertRaisesRegex(RuntimeError, "dependencies changed"):
                decoder.decode_with_provenance(Path("unused"))

    def test_preloaded_extension_cannot_be_identified_from_current_disk_bytes(self):
        with patch.dict("sys.modules", {"_vmprof": types.ModuleType("_vmprof")}):
            with self.assertRaisesRegex(RuntimeError, "fresh process"):
                decoder.decode_with_provenance(Path("unused"))

    def test_timestamp_valid_stale_bytecode_cannot_replace_identified_reader_source(
        self,
    ):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            package = root / "vmprof"
            package.mkdir()
            (package / "__init__.py").write_text("")
            metadata = root / "vmprof-0.6.dist-info"
            metadata.mkdir()
            (metadata / "METADATA").write_text("Name: vmprof\nVersion: 0.6\n")
            reader = package / "reader.py"
            reader.write_text("""DEFAULT_MAX_SAMPLE_GAP = 0
def gunzip(source): return source
class LogReaderState:
    def __init__(self, gap):
        self.virtual_ips = []
        self.meta = {"os": "linux"}
        self.profile_real_time = False
        self.profiles = []
        self.period = 1
        self.lost_time = 0
class LogReader:
    def __init__(self, source, state): pass
    def read_all(self): pass
""")
            old = reader.stat()
            py_compile.compile(str(reader), doraise=True)
            reader.write_text(reader.read_text().replace('"linux"', '"win32"'))
            os.utime(reader, ns=(old.st_atime_ns, old.st_mtime_ns))
            self.assertEqual(reader.stat().st_size, old.st_size)
            raw = root / "control.vmprof"
            raw.write_bytes(b"")
            result = subprocess.run(
                [sys.executable, str(Path(decoder.__file__).resolve()), str(raw)],
                env=dict(os.environ, PYTHONPATH=str(root)),
                capture_output=True,
                text=True,
                timeout=15,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(json.loads(result.stdout)["clock"], "runtime_monotonic")


if __name__ == "__main__":
    unittest.main()
