"""Real interpreter controls for bounded automatic Python call counting."""

import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest


ROOT = Path(__file__).resolve().parent


class PythonCountTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.python = os.environ.get("HUNIN_TEST_COUNTS_PYTHON", sys.executable)
        cls.temporary = tempfile.TemporaryDirectory()
        cls.cache = Path(cls.temporary.name) / "cache"
        probe = subprocess.run(
            [cls.python, "-c", "import sys;print(int(hasattr(sys,'monitoring')))"],
            capture_output=True,
            text=True,
            timeout=10,
            check=True,
        )
        if probe.stdout.strip() != "1":
            cls.temporary.cleanup()
            raise unittest.SkipTest("sys.monitoring requires CPython 3.12 or later")
        compiled = subprocess.run(
            [cls.python, str(ROOT / "build.py"), str(cls.cache)],
            capture_output=True,
            text=True,
            timeout=30,
        )
        if compiled.returncode:
            raise AssertionError(compiled.stderr)
        cls.extension = json.loads(compiled.stdout)["extension"]

    @classmethod
    def tearDownClass(cls):
        cls.temporary.cleanup()

    def capture(self, target, *, functions=None, max_functions=128):
        directory = Path(tempfile.mkdtemp(dir=self.temporary.name))
        config = {
            "run_id": "count-control",
            "output": str(directory / "calls.calls.jsonl"),
            "completion": str(directory / "calls.complete.json"),
            "extension": self.extension,
            "functions": functions or [],
            "max_functions": max_functions,
            "max_output_bytes": 65536,
        }
        path = directory / "config.json"
        path.write_text(json.dumps(config))
        result = subprocess.run(
            [self.python, str(ROOT / "python.py"), str(path), "--", *target],
            cwd=directory,
            capture_output=True,
            text=True,
            timeout=10,
        )
        completion = json.loads(Path(config["completion"]).read_text())
        output = Path(config["output"])
        records = (
            [json.loads(line) for line in output.read_text().splitlines()]
            if output.exists()
            else []
        )
        return result, completion, records

    def test_starts_resumes_recursion_threads_and_atexit_have_independent_counts(self):
        code = """import atexit,threading
def leaf(): return 7
def recur(n): return 1 if n == 0 else 1 + recur(n-1)
def gen():
    for n in range(3): yield n
def worker():
    for _ in range(11): leaf()
t=threading.Thread(target=worker);t.start();t.join()
atexit.register(lambda: (leaf(),leaf()))
print(recur(6),sum(gen()))
"""
        result, completion, records = self.capture(
            ["-c", code],
            functions=[{"name": name} for name in ("leaf", "recur", "gen")],
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout, "7 3\n")
        self.assertEqual(completion["status"], "complete")
        counts = {
            (
                r["attributes"]["function"]["name"],
                r["attributes"]["count_semantics"],
            ): r["value"]
            for r in records
        }
        self.assertEqual(counts[("leaf", "starts")], 13)
        self.assertEqual(counts[("recur", "starts")], 7)
        self.assertEqual(counts[("gen", "starts")], 1)
        self.assertEqual(counts[("gen", "resumes")], 3)
        self.assertTrue(
            all(r["attributes"]["clock_alignment"] == "direct_epoch" for r in records)
        )

    def test_eval_entry_preserves_argv_cwd_and_main_loader(self):
        code = "import json,sys,os;print(json.dumps([sys.argv,os.getcwd(),__name__,__loader__.__name__]))"
        result, completion, _ = self.capture(["-c", code, "argument with spaces"])
        self.assertEqual(result.returncode, 0, result.stderr)
        value = json.loads(result.stdout)
        self.assertEqual(value[0], ["-c", "argument with spaces"])
        self.assertEqual(value[2:], ["__main__", "BuiltinImporter"])
        self.assertEqual(completion["status"], "complete")

    def test_table_overflow_preserves_target_but_marks_counts_incomplete(self):
        result, completion, _ = self.capture(
            ["-c", "def leaf(): pass\nleaf();print('ok')"], max_functions=1
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout, "ok\n")
        self.assertEqual(completion["status"], "incomplete")
        self.assertGreater(completion["dropped_events"], 0)

    def test_abrupt_exit_never_publishes_a_successful_empty_count_set(self):
        result, completion, records = self.capture(["-c", "import os;os._exit(17)"])
        self.assertEqual(result.returncode, 17)
        self.assertEqual(completion["status"], "running")
        self.assertEqual(records, [])

    def test_unhandled_target_error_remains_nonzero_and_incomplete(self):
        result, completion, _ = self.capture(
            ["-c", "raise ValueError('original failure')"]
        )
        self.assertEqual(result.returncode, 1)
        self.assertIn("original failure", result.stderr)
        self.assertEqual(completion["status"], "incomplete")


if __name__ == "__main__":
    unittest.main()
