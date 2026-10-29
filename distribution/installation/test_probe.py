import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

from . import bundle as installer


class ProbeOutputTests(unittest.TestCase):
    def run_control(self, code, timeout=10):
        children = []
        spawn = subprocess.Popen

        def tracked_spawn(*args, **kwargs):
            process = spawn(*args, **kwargs)
            children.append(process)
            return process

        with (
            tempfile.TemporaryDirectory() as work,
            patch.object(installer.subprocess, "Popen", side_effect=tracked_spawn),
        ):
            try:
                return installer._probe_output(
                    [sys.executable, "-c", code], work, dict(os.environ), timeout
                )
            finally:
                self.assertTrue(children)
                self.assertTrue(all(process.poll() is not None for process in children))

    def test_oversized_actual_byte_streams_are_stopped_and_reaped(self):
        for stream in ("stdout", "stderr"):
            with self.subTest(stream=stream):
                # 20,000 Unicode characters are 80,000 UTF-8 bytes.
                code = f"import sys,time;sys.{stream}.buffer.write(('😀'*20000).encode());sys.{stream}.buffer.flush();time.sleep(30)"
                with self.assertRaisesRegex(ValueError, "byte limit"):
                    self.run_control(code)

    def test_stdout_and_stderr_share_one_total_budget(self):
        code = "import sys,time;sys.stdout.buffer.write(b'A'*32769);sys.stdout.buffer.flush();sys.stderr.buffer.write(b'B'*32769);sys.stderr.buffer.flush();time.sleep(30)"
        with self.assertRaisesRegex(ValueError, "combined stdout/stderr"):
            self.run_control(code)

    def test_silent_timeout_reaps_the_actual_child(self):
        with self.assertRaisesRegex(ValueError, "timed out"):
            self.run_control("import time;time.sleep(30)", timeout=0.1)

    @unittest.skipIf(
        os.name == "nt",
        "POSIX shebang adapter; pipe controls above use actual Python on every host",
    )
    def test_failure_diagnostic_retains_at_most_two_thousand_bytes(self):
        with tempfile.TemporaryDirectory() as temporary:
            executable = Path(temporary) / "controlled executable"
            executable.write_text(
                f"#!{sys.executable}\nimport sys\nsys.stderr.buffer.write(b'Z'*5000)\nsys.exit(7)\n"
            )
            executable.chmod(0o755)
            with self.assertRaisesRegex(ValueError, r"failed \(7\)") as failure:
                installer.probe_binary(executable)
            self.assertEqual(str(failure.exception).count("Z"), 2000)
