import json
from pathlib import Path
import tempfile
import unittest

from .verification import fixture_from_recording


class SymbolReaderInputTest(unittest.TestCase):
    def test_recording_input_requires_the_completed_correct_go_test_route(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            run = {
                "id": "controlled",
                "runtime": "go-test",
                "status": "completed",
                "correctness": {"passed": True},
            }
            manifest = root / "run.json"
            manifest.write_text(json.dumps({"run": run}))
            fixture, provenance = fixture_from_recording(root)
            self.assertEqual(
                fixture, root / "recordings/captures/controlled/workload.test.exe"
            )
            self.assertEqual(provenance["run_id"], "controlled")
            self.assertEqual(len(provenance["run_manifest_sha256"]), 64)
            for change in (
                {"runtime": "go-trace"},
                {"status": "failed"},
                {"correctness": {"passed": None}},
                {"id": "../foreign"},
            ):
                with self.subTest(change=change):
                    manifest.write_text(json.dumps({"run": {**run, **change}}))
                    with self.assertRaises(ValueError):
                        fixture_from_recording(root)


if __name__ == "__main__":
    unittest.main()
