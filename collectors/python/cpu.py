"""Sample Python stacks only when measured OS thread CPU counters advance.

Weights describe CPU progress across a bounded observation window. They do not
measure individual calls. Delayed samples are retained as unattributed evidence.
"""

import hashlib
import json
import marshal
import os
import signal
from pathlib import Path
import sys
import threading
import time
import weakref
from cpu_clock import clock
from entry import run_target


class Sampler:
    def __init__(self, path, interval=0.01, max_gap=0.1, max_latency=0.01):
        self.path = path
        self.clock = clock()
        self.interval = interval
        self.max_gap_ns = int(max_gap * 1_000_000_000)
        self.max_latency_ns = int(max_latency * 1_000_000_000)
        self.stopped = threading.Event()
        self.thread = None
        self.previous = {}
        self.codes = weakref.WeakKeyDictionary()
        self.sequence = 0
        self.observed_cpu_ns = 0
        self.attributed_cpu_ns = 0
        self.gaps = 0
        self.ended_thread_gaps = 0
        self.errors = []
        self.output = path.open("w", encoding="utf8")
        before = self.clock.now_ns()
        epoch = time.time_ns()
        after = self.clock.now_ns()
        self.metadata = {
            "format": "python-cpu-progress-v1",
            "pid": os.getpid(),
            "interpreter": sys.executable,
            "python_version": sys.version,
            "clock_source": self.clock.source,
            "epoch_origin_ns": epoch - (before + after) // 2,
            "clock_uncertainty_ns": (after - before + 1) // 2,
            "interval_ns": int(interval * 1_000_000_000),
            "max_gap_ns": self.max_gap_ns,
            "max_capture_latency_ns": self.max_latency_ns,
            "capture_scope": "one Python thread stack and its OS thread CPU counter read",
            "max_stack_frames": 512,
            "weight_semantics": "OS thread CPU counter delta across preceding window, statistically attributed to captured Python stack; not a call duration",
            "limitations": [
                "Python GIL scheduling and OS counter resolution limit sample coverage.",
                "Long GIL-holding native calls and delayed observations are retained as unattributed CPU, not assigned to a later frame.",
                "Only discovered Python threads with native IDs are covered; native-only threads and short-lived threads can be absent.",
            ],
        }

    def emit(self, value):
        self.output.write(json.dumps(value, ensure_ascii=True) + "\n")

    def stack(self, frame):
        result = []
        while frame is not None and len(result) < 512:
            code = frame.f_code
            identity = self.codes.get(code)
            if identity is None:
                identity = hashlib.sha256(marshal.dumps(code)).hexdigest()
                self.codes[code] = identity
            result.append(
                {
                    "function": code.co_name,
                    "file": code.co_filename,
                    "line": frame.f_lineno,
                    "module": frame.f_globals.get("__name__", ""),
                    "code_id": identity,
                }
            )
            frame = frame.f_back
        return list(reversed(result))

    def observe(self, final=False):
        sampler_id = self.thread.ident if self.thread is not None else None
        threads = {
            thread.ident: thread.native_id
            for thread in threading.enumerate()
            if thread.ident is not None
            and thread.ident != sampler_id
            and getattr(thread, "native_id", None) is not None
        }
        current_ids = set(threads.values())
        # Discover/release OS handles once using the whole live thread set.
        # Bracket each frame+counter read independently: an unrelated thread's
        # FFI/GIL delay must not enlarge another thread's attribution window.
        self.clock.retain(current_ids)
        self.previous = {
            key: value for key, value in self.previous.items() if key[0] in current_ids
        }
        self.sequence += 1
        for ident, native_id in threads.items():
            started = self.clock.now_ns()
            frame = None if final else sys._current_frames().get(ident)
            stack = None if frame is None else self.stack(frame)
            captured = self.clock.now_ns()
            counter = self.clock.snapshot({native_id}).get(native_id)
            finished = self.clock.now_ns()
            for ended in getattr(self.clock, "take_ended_counters", lambda: [])():
                self.emit(
                    {
                        "record_type": "unmapped_thread_end_counter",
                        "timestamp_ns": finished,
                        "reason": "cached OS thread terminated; Python identity and final CPU delta remain unmapped",
                        **ended,
                    }
                )
                self.ended_thread_gaps += 1
            if counter is None:
                continue
            key = (native_id, counter["lifetime"])
            previous = self.previous.get(key)
            self.previous[key] = (finished, counter["cpu_ns"])
            delta = 0 if previous is None else counter["cpu_ns"] - previous[1]
            interval = 0 if previous is None else finished - previous[0]
            latency = finished - started
            if previous is None:
                attribution = "baseline"
            elif delta < 0:
                attribution = "counter_reset"
            elif delta == 0:
                attribution = "idle"
            elif final or interval > self.max_gap_ns or latency > self.max_latency_ns:
                attribution = "unattributed_gap"
                self.gaps += 1
            elif stack is None:
                attribution = "missing_frame"
            else:
                attribution = "sample"
                self.attributed_cpu_ns += delta
            self.observed_cpu_ns += max(0, delta)
            self.emit(
                {
                    "record_type": "observation",
                    "sequence": self.sequence,
                    "thread_ident": str(ident),
                    "native_tid": native_id,
                    "thread_lifetime": counter["lifetime"],
                    "timestamp_ns": captured,
                    "frame_capture_started_ns": started,
                    "frame_capture_finished_ns": captured,
                    "read_finished_ns": finished,
                    "cpu_counter_ns": counter["cpu_ns"],
                    "cpu_delta_ns": max(0, delta),
                    "interval_ns": interval,
                    "capture_latency_ns": latency,
                    "attribution": attribution,
                    "stack": stack or [],
                }
            )

    def loop(self):
        try:
            while not self.stopped.wait(self.interval):
                self.observe()
        except BaseException as error:
            self.errors.append(f"{type(error).__name__}: {error}")

    def start(self):
        self.observe()
        self.thread = threading.Thread(
            target=self.loop, name="hunin-cpu-sampler", daemon=True
        )
        self.thread.start()

    def finish(self):
        self.stopped.set()
        if self.thread is not None:
            self.thread.join()
        try:
            self.observe(final=True)
        except BaseException as error:
            self.errors.append(f"{type(error).__name__}: {error}")
        finally:
            self.clock.close()
            self.output.close()
        self.metadata.update(
            observed_cpu_ns=self.observed_cpu_ns,
            attributed_cpu_ns=self.attributed_cpu_ns,
            unattributed_cpu_ns=self.observed_cpu_ns - self.attributed_cpu_ns,
            gap_observations=self.gaps,
            errors=self.errors,
            unmapped_ended_thread_counters=self.ended_thread_gaps,
            coverage=self.attributed_cpu_ns / self.observed_cpu_ns
            if self.observed_cpu_ns
            else None,
        )
        self.path.with_suffix(self.path.suffix + ".meta.json").write_text(
            json.dumps(self.metadata), encoding="utf8"
        )


def main():
    path = Path(sys.argv[1]).resolve()
    target = sys.argv[2:]
    if target[:1] == ["--"]:
        target.pop(0)
    if not target:
        raise SystemExit("a Python script, -m module or -c code is required")
    if (
        hasattr(signal, "SIGBREAK")
        and signal.getsignal(signal.SIGBREAK) == signal.SIG_DFL
    ):
        # ProcessTree uses CTRL_BREAK on Windows. Let the collector's finally
        # block flush counters/metadata; the target can install its own handler.
        signal.signal(signal.SIGBREAK, signal.default_int_handler)
    sampler = Sampler(path)
    sampler.start()
    try:
        run_target(target)
    finally:
        sampler.finish()


if __name__ == "__main__":
    main()
