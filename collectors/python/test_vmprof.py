import json
import pathlib
import subprocess
import sys
import tempfile
import unittest


HERE = pathlib.Path(__file__).parent


class VmprofCaptureTest(unittest.TestCase):
    def test_script_and_module_keep_native_exit_namespace_and_flush_raw_profile(self):
        source = (
            "import atexit, json, sys\n"
            "marker = 'target main survives capture'\n"
            "def on_exit():\n"
            "    import __main__\n"
            "    print(json.dumps({'marker': __main__.marker, 'same_globals': vars(__main__) is globals(), 'package': __package__, 'argv': sys.argv}))\n"
            "atexit.register(on_exit)\n"
            "raise SystemExit(7)\n"
        )
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            (root / "target.py").write_text(source)
            (root / "entry_package").mkdir()
            (root / "entry_package/__init__.py").write_text("")
            (root / "entry_package/main.py").write_text(source)
            for label, target in [
                ("script", ["target.py"]),
                ("module", ["-m", "entry_package.main"]),
            ]:
                with self.subTest(target=label):
                    profile = root / (label + ".vmprof")
                    arguments = [*target, "argument with spaces"]
                    native = subprocess.run(
                        [sys.executable, *arguments],
                        cwd=root,
                        capture_output=True,
                        timeout=30,
                    )
                    captured = subprocess.run(
                        [
                            sys.executable,
                            str(HERE.resolve() / "capture.py"),
                            str(profile),
                            "--",
                            *arguments,
                        ],
                        cwd=root,
                        capture_output=True,
                        timeout=30,
                    )
                    self.assertEqual(native.returncode, 7, native.stderr)
                    self.assertEqual(captured.returncode, 7, captured.stderr)
                    self.assertEqual(captured.stdout, native.stdout)
                    self.assertEqual(captured.stderr, native.stderr)
                    self.assertGreater(profile.stat().st_size, 0)
                    self.assertTrue(profile.with_suffix(".vmprof.meta.json").exists())

    def test_command_main_survives_exit_callbacks_and_flushes_failed_capture(self):
        with tempfile.TemporaryDirectory() as directory:
            profile = pathlib.Path(directory) / "command.vmprof"
            source = (
                "import atexit\n"
                "marker = 'target main'\n"
                "def on_exit():\n"
                "    import __main__\n"
                "    print(__main__.marker)\n"
                "atexit.register(on_exit)\n"
                "raise SystemExit(7)\n"
            )
            result = subprocess.run(
                [
                    sys.executable,
                    str(HERE.resolve() / "capture.py"),
                    str(profile),
                    "--",
                    "-c",
                    source,
                ],
                capture_output=True,
                text=True,
                timeout=30,
            )
            self.assertEqual(result.returncode, 7, result.stderr)
            self.assertEqual(result.stdout, "target main\n")
            self.assertEqual(result.stderr, "")
            self.assertGreater(profile.stat().st_size, 0)
            metadata = json.loads(profile.with_suffix(".vmprof.meta.json").read_text())
            self.assertEqual(metadata["interpreter"], sys.executable)
            self.assertGreaterEqual(
                metadata["epoch_end_us"], metadata["epoch_start_us"]
            )

    def test_module_entry_uses_workload_directory_and_preserves_failure(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            (root / "control_module.py").write_text(
                "import sys\nassert sys.argv[1:] == ['argument']\nraise SystemExit(7)\n"
            )
            profile = root / "module.vmprof"
            result = subprocess.run(
                [
                    sys.executable,
                    str(HERE.resolve() / "capture.py"),
                    str(profile),
                    "--",
                    "-m",
                    "control_module",
                    "argument",
                ],
                cwd=root,
                capture_output=True,
                text=True,
            )
            self.assertEqual(result.returncode, 7, result.stderr)
            self.assertEqual(
                result.stdout, "", "collector setup must not add output to the workload"
            )
            self.assertTrue(profile.exists())
            self.assertTrue(profile.with_suffix(".vmprof.meta.json").exists())

    def test_cpu_capture_preserves_source_and_declared_clock(self):
        with tempfile.TemporaryDirectory() as directory:
            profile = pathlib.Path(directory) / "cpu.vmprof"
            capture = subprocess.run(
                [
                    sys.executable,
                    str(HERE / "capture.py"),
                    str(profile),
                    "--",
                    str(HERE / "fixtures" / "cpu_work.py"),
                ],
                capture_output=True,
                text=True,
            )
            self.assertEqual(capture.returncode, 0, capture.stderr)
            result = subprocess.run(
                [sys.executable, str(HERE / "decode.py"), str(profile)],
                capture_output=True,
                text=True,
                check=True,
            )
            decoded = json.loads(result.stdout)
            self.assertGreater(decoded["pid"], 0)
            self.assertEqual(
                decoded["clock"],
                "runtime_monotonic" if sys.platform == "win32" else "process_cpu",
            )
            self.assertEqual(decoded["clock_alignment"], "unmapped")
            self.assertTrue(decoded["samples"])
            frames = [
                frame for sample in decoded["samples"] for frame in sample["stack"]
            ]
            self.assertTrue(
                any(frame["function"] == "hunin_python_hotspot" for frame in frames)
            )
            self.assertTrue(
                any(frame.get("file", "").endswith("cpu_work.py") for frame in frames)
            )


if __name__ == "__main__":
    unittest.main()
