import ctypes
import os
import time


class LinuxClock:
    source = "python_perf_counter_monotonic"

    def __init__(self):
        self.frequency = os.sysconf("SC_CLK_TCK")
        # Python's open/read/close each release the GIL. With CPU-bound
        # Python threads, reacquiring it between those calls can make an
        # otherwise tiny counter read take many scheduling quanta. Keep
        # CPython's GIL for one bounded procfs pread instead; PyPy's CDLL
        # fallback remains subject to the same measured latency gate.
        self.api = getattr(ctypes, "PyDLL", ctypes.CDLL)(None)
        self.api.open.argtypes = [ctypes.c_char_p, ctypes.c_int]
        self.api.open.restype = ctypes.c_int
        self.api.pread.argtypes = [
            ctypes.c_int,
            ctypes.c_void_p,
            ctypes.c_size_t,
            ctypes.c_longlong,
        ]
        self.api.pread.restype = ctypes.c_ssize_t
        self.api.close.argtypes = [ctypes.c_int]
        self.api.close.restype = ctypes.c_int
        self.handles = {}
        self.buffer = ctypes.create_string_buffer(4096)

    def now_ns(self):
        return time.perf_counter_ns()

    def retain(self, native_ids):
        for tid in set(self.handles) - native_ids:
            self.api.close(self.handles.pop(tid))
        for tid in native_ids:
            if tid not in self.handles:
                handle = self.api.open(
                    f"/proc/self/task/{tid}/stat".encode(), os.O_RDONLY | os.O_CLOEXEC
                )
                if handle >= 0:
                    self.handles[tid] = handle

    def snapshot(self, native_ids):
        self.retain(set(self.handles) | native_ids)
        result = {}
        for tid in native_ids:
            try:
                handle = self.handles.get(tid)
                if handle is None:
                    continue
                count = self.api.pread(handle, self.buffer, len(self.buffer), 0)
                if count <= 0 or count == len(self.buffer):
                    self.api.close(self.handles.pop(tid))
                    continue
                fields = self.buffer.raw[:count].rsplit(b")", 1)[1].split()
                result[tid] = {
                    "cpu_ns": (int(fields[11]) + int(fields[12]))
                    * 1_000_000_000
                    // self.frequency,
                    "lifetime": fields[19].decode("ascii"),
                }
            except (OSError, ValueError, IndexError):
                continue
        return result

    def close(self):
        for handle in self.handles.values():
            self.api.close(handle)
        self.handles.clear()
