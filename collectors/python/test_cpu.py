import json
from contextlib import nullcontext
import os
import signal
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import unittest
from unittest.mock import patch


HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))


def read_observations(path):
    rows = [json.loads(line) for line in path.read_text().splitlines()]
    return [row for row in rows if row["record_type"] == "observation"]


class CpuProgressTest(unittest.TestCase):
    def test_capture_helpers_do_not_shadow_workload_modules(self):
        for target in (["main.py"], ["-m", "main"], ["-c", "import main"]):
            with (
                self.subTest(target=target),
                tempfile.TemporaryDirectory() as directory,
            ):
                work = Path(directory)
                for name in ("entry", "cpu_clock"):
                    (work / f"{name}.py").write_text(f"value = {name!r}\n")
                (work / "main.py").write_text(
                    "import entry, cpu_clock\n"
                    "assert entry.value == 'entry'\n"
                    "assert cpu_clock.value == 'cpu_clock'\n"
                    "print('workload imports preserved')\n"
                )
                result = subprocess.run(
                    [
                        sys.executable,
                        str(HERE / "cpu.py"),
                        str(work / "cpu.pythoncpu.jsonl"),
                        "--",
                        *target,
                    ],
                    cwd=work,
                    capture_output=True,
                    text=True,
                )
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(result.stdout, "workload imports preserved\n")

    def test_thread_end_counter_is_retained_without_stack_attribution(self):
        # Windows can observe a terminated cached thread handle between frame
        # discovery and its counter read. That raw counter has no stack or
        # attribution; it must not enter the positive/negative sample gates.
        sample = {
            "record_type": "observation",
            "attribution": "sample",
            "cpu_delta_ns": 15_625_000,
        }
        ended = {
            "record_type": "unmapped_thread_end_counter",
            "native_tid": 5628,
            "thread_lifetime": "134372610373580930",
            "cpu_counter_ns": 0,
            "ended_filetime_100ns": 134372610378897921,
        }
        with tempfile.TemporaryDirectory(prefix="hunin-ended-counter-") as directory:
            raw = Path(directory) / "cpu.pythoncpu.jsonl"
            raw.write_text("\n".join(json.dumps(row) for row in [sample, ended]) + "\n")
            observations = read_observations(raw)
            self.assertEqual(
                sum(
                    row["cpu_delta_ns"]
                    for row in observations
                    if row["attribution"] == "sample"
                ),
                15_625_000,
            )
            self.assertEqual(
                [json.loads(line) for line in raw.read_text().splitlines()][1], ended
            )

    def test_terminated_cached_windows_handle_is_not_reused_for_a_python_frame(self):
        from ctypes import wintypes
        from cpu_clock import WindowsClock

        class Api:
            def __init__(self):
                self.closed = []

            def GetThreadTimes(self, handle, created, ended, kernel, user):
                created._obj.dwLowDateTime = 17
                ended._obj.dwLowDateTime = 99
                kernel._obj.dwLowDateTime = 5
                user._obj.dwLowDateTime = 7
                return True

            def CloseHandle(self, handle):
                self.closed.append(handle)

        clock = WindowsClock.__new__(WindowsClock)
        clock.api = Api()
        clock.handles = {8: "terminated-handle"}
        clock.filetime = wintypes.FILETIME
        clock.unmapped_ended = []
        self.assertEqual(clock.snapshot({8}), {})
        self.assertEqual(clock.handles, {})
        self.assertEqual(clock.api.closed, ["terminated-handle"])
        self.assertEqual(
            clock.take_ended_counters(),
            [
                {
                    "native_tid": 8,
                    "thread_lifetime": "17",
                    "cpu_counter_ns": 1200,
                    "ended_filetime_100ns": 99,
                }
            ],
        )
        self.assertEqual(clock.take_ended_counters(), [])

    def test_owned_interrupt_flushes_raw_counters_and_metadata(self):
        with tempfile.TemporaryDirectory(prefix="hunin-cpu-cancel-") as directory:
            work = Path(directory)
            script = work / "work.py"
            script.write_text(
                "from pathlib import Path\nPath('ready').write_text('ready')\nwhile True: sum(range(10000))\n"
            )
            raw = work / "cpu.pythoncpu.jsonl"
            options = (
                {"creationflags": subprocess.CREATE_NEW_PROCESS_GROUP}
                if os.name == "nt"
                else {}
            )
            child = subprocess.Popen(
                [sys.executable, str(HERE / "cpu.py"), str(raw), "--", str(script)],
                cwd=work,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                **options,
            )
            try:
                deadline = time.monotonic() + 5
                while not (work / "ready").exists() and time.monotonic() < deadline:
                    time.sleep(0.01)
                self.assertTrue(
                    (work / "ready").exists(), "controlled target never started"
                )
                child.send_signal(
                    signal.CTRL_BREAK_EVENT if os.name == "nt" else signal.SIGINT
                )
                child.communicate(timeout=5)
                metadata = json.loads(
                    raw.with_suffix(raw.suffix + ".meta.json").read_text()
                )
                self.assertNotEqual(child.returncode, 0)
                self.assertFalse(metadata["errors"])
                self.assertTrue(raw.read_text().strip())
            finally:
                if child.poll() is None:
                    child.kill()
                    child.wait()

    def capture(self, source):
        evidence = os.environ.get("HUNIN_PYTHON_CPU_EVIDENCE")
        if evidence:
            root = Path(evidence).resolve()
            root.mkdir(parents=True, exist_ok=True)
            directory = tempfile.mkdtemp(prefix=self._testMethodName + "-", dir=root)
            capture_directory = nullcontext(directory)
        else:
            capture_directory = tempfile.TemporaryDirectory(
                prefix="hunin-cpu-progress-"
            )
        with capture_directory as directory:
            work = Path(directory)
            script = work / "work.py"
            script.write_text(source)
            raw = work / "cpu.pythoncpu.jsonl"
            result = subprocess.run(
                [sys.executable, str(HERE / "cpu.py"), str(raw), "--", str(script)],
                capture_output=True,
                text=True,
                timeout=30,
            )
            (work / "stdout.txt").write_text(result.stdout)
            (work / "stderr.txt").write_text(result.stderr)
            (work / "execution.json").write_text(
                json.dumps(
                    {
                        "command": result.args,
                        "returncode": result.returncode,
                        "interpreter": sys.version,
                    }
                )
            )
            if evidence:
                print(f"CPU progress evidence: {work}", file=sys.stderr)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(result.stdout, "expected\n")
            rows = read_observations(raw)
            metadata = json.loads(
                raw.with_suffix(raw.suffix + ".meta.json").read_text()
            )
            self.workload = (
                json.loads((work / "workload.json").read_text())
                if (work / "workload.json").exists()
                else None
            )
            self.assertFalse(metadata["errors"])
            return rows, metadata

    def test_failed_capture_preserves_raw_execution_and_target_evidence(self):
        with tempfile.TemporaryDirectory(prefix="hunin-cpu-evidence-") as directory:
            with patch.dict(os.environ, {"HUNIN_PYTHON_CPU_EVIDENCE": directory}):
                with self.assertRaises(AssertionError):
                    self.capture("print('unexpected stdout')\n")
            (work,) = Path(directory).iterdir()
            self.assertTrue((work / "cpu.pythoncpu.jsonl").is_file())
            self.assertTrue((work / "cpu.pythoncpu.jsonl.meta.json").is_file())
            self.assertEqual((work / "stdout.txt").read_text(), "unexpected stdout\n")
            self.assertEqual((work / "stderr.txt").read_text(), "")
            self.assertEqual(
                json.loads((work / "execution.json").read_text())["returncode"], 0
            )
            self.assertTrue((work / "work.py").is_file())

    def test_measured_cpu_progress_distinguishes_hot_work_idle_and_thread_identity(
        self,
    ):
        rows, metadata = self.capture("""
import json,threading,time
from pathlib import Path
ready=threading.Barrier(2)
windows=[]
def cpu_hot():
    ready.wait()
    cpu_start=time.thread_time()
    wall_start=time.perf_counter()
    end=cpu_start+0.45
    next_yield=cpu_start+0.005
    scheduling_yields=0
    value=1
    while time.thread_time()<end:
        for n in range(10000): value=(value+n)%10000007
        now=time.thread_time()
        if now>=next_yield:
            # This positive control supplies bounded observation opportunities.
            # The separate long-GIL control deliberately supplies none.
            time.sleep(0.001)
            scheduling_yields+=1
            next_yield=now+0.005
    windows.append({'native_tid':threading.get_native_id(),'thread_cpu_seconds':time.thread_time()-cpu_start,'wall_start_seconds':wall_start,'wall_end_seconds':time.perf_counter(),'scheduling_yields':scheduling_yields,'yield_every_thread_cpu_seconds':0.005})
def idle_wait(): time.sleep(0.5)
hot=threading.Thread(target=cpu_hot)
idle=threading.Thread(target=idle_wait)
hot.start(); idle.start(); cpu_hot(); hot.join(); idle.join()
Path(__file__).with_name('workload.json').write_text(json.dumps(windows))
print('expected')
""")
        samples = [row for row in rows if row["attribution"] == "sample"]
        self.assertTrue(samples)
        cpu = sum(
            row["cpu_delta_ns"]
            for row in samples
            if any(frame["function"] == "cpu_hot" for frame in row["stack"])
        )
        idle = sum(
            row["cpu_delta_ns"]
            for row in samples
            if any(frame["function"] == "idle_wait" for frame in row["stack"])
        )
        self.assertGreater(cpu, 0)
        self.assertLess(idle, cpu * 0.1)
        hot_threads = {
            row["native_tid"]
            for row in samples
            if any(frame["function"] == "cpu_hot" for frame in row["stack"])
        }
        self.assertGreaterEqual(
            len(hot_threads),
            2,
            f"captured hot native TIDs: {sorted(hot_threads)}; raw evidence: {os.environ.get('HUNIN_PYTHON_CPU_EVIDENCE', 'not configured')}",
        )
        self.assertEqual(
            hot_threads, {window["native_tid"] for window in self.workload}
        )
        self.assertEqual(len(self.workload), 2)
        self.assertTrue(
            all(window["thread_cpu_seconds"] >= 0.45 for window in self.workload)
        )
        self.assertTrue(
            all(window["scheduling_yields"] > 0 for window in self.workload)
        )
        self.assertTrue(
            all(row["thread_ident"] and row["thread_lifetime"] for row in samples)
        )
        self.assertGreaterEqual(
            metadata["observed_cpu_ns"], metadata["attributed_cpu_ns"]
        )
        self.assertGreater(metadata["attributed_cpu_ns"], 0)

    def test_long_gil_holding_native_call_is_not_attributed_to_a_later_idle_frame(self):
        with tempfile.TemporaryDirectory(prefix="hunin-native-gil-") as directory:
            library = Path(directory) / (
                "hold.dll"
                if os.name == "nt"
                else "libhold.dylib"
                if sys.platform == "darwin"
                else "libhold.so"
            )
            build = subprocess.run(
                [
                    "rustc",
                    "--crate-type",
                    "cdylib",
                    "-C",
                    "opt-level=1",
                    str(HERE / "fixtures" / "cpu_native.rs"),
                    "-o",
                    str(library),
                ],
                capture_output=True,
                text=True,
                timeout=30,
            )
            self.assertEqual(build.returncode, 0, build.stderr)
            source = """
import ctypes,time
library=ctypes.PyDLL(%r)
library.cpu_hold.restype=ctypes.c_uint64
def long_native_hold(): library.cpu_hold()
def later_idle(): time.sleep(0.15)
long_native_hold(); later_idle()
print('expected')
""" % str(library)
            if sys.implementation.name == "pypy":
                # PyPy ctypes releases the GIL for native calls and has no
                # PyDLL. Exercise the same delayed-observation boundary with
                # a deliberately extended interpreter scheduling interval.
                source = """
import sys,time
sys.setswitchinterval(1.0)
def long_interpreter_hold():
    end=time.perf_counter()+0.4
    value=1
    while time.perf_counter()<end:
        for n in range(10000): value=(value+n)%10000007
def later_idle(): time.sleep(0.15)
long_interpreter_hold(); later_idle()
print('expected')
"""
            rows, metadata = self.capture(source)
        gaps = [
            row
            for row in rows
            if row["attribution"] == "unattributed_gap" and row["cpu_delta_ns"] > 0
        ]
        self.assertTrue(gaps, "long GIL hold must leave unassigned CPU evidence")
        self.assertGreater(metadata["unattributed_cpu_ns"], 100_000_000)
        late = sum(
            row["cpu_delta_ns"]
            for row in rows
            if row["attribution"] == "sample"
            and any(frame["function"] == "later_idle" for frame in row["stack"])
        )
        self.assertLess(late, metadata["unattributed_cpu_ns"] * 0.1)


if __name__ == "__main__":
    unittest.main()
