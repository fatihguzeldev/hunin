"""Read cumulative OS thread CPU counters without tracing Python calls."""

import ctypes
import os
import sys
import time


class WindowsClock:
    def __init__(self):
        from ctypes import wintypes
        self.api = getattr(ctypes, "PyDLL", ctypes.CDLL)("kernel32", use_last_error=True)
        self.api.OpenThread.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
        self.api.OpenThread.restype = wintypes.HANDLE
        self.api.CloseHandle.argtypes = [wintypes.HANDLE]
        self.api.GetThreadTimes.argtypes = [wintypes.HANDLE] + [ctypes.POINTER(wintypes.FILETIME)] * 4
        self.api.GetThreadTimes.restype = wintypes.BOOL
        self.source = "python_perf_counter:" + time.get_clock_info("perf_counter").implementation
        self.handles = {}
        self.filetime = wintypes.FILETIME

    def now_ns(self):
        # Both supported interpreters implement this with QPC on Windows.
        # Calling their builtin avoids an extra PyPy ctypes GIL handoff.
        return time.perf_counter_ns()

    def snapshot(self, native_ids):
        result = {}
        for tid in set(self.handles) - native_ids:
            self.api.CloseHandle(self.handles.pop(tid))
        for tid in native_ids:
            handle = self.handles.get(tid)
            if handle is None:
                handle = self.api.OpenThread(0x0800, False, tid)  # THREAD_QUERY_LIMITED_INFORMATION
                if not handle:
                    continue
                self.handles[tid] = handle
            created, ended, kernel, user = (self.filetime() for _ in range(4))
            if self.api.GetThreadTimes(handle, ctypes.byref(created), ctypes.byref(ended), ctypes.byref(kernel), ctypes.byref(user)):
                ticks = lambda item: (item.dwHighDateTime << 32) | item.dwLowDateTime
                result[tid] = {"cpu_ns": (ticks(kernel) + ticks(user)) * 100,
                               "lifetime": str(ticks(created))}
        return result

    def close(self):
        for handle in self.handles.values():
            self.api.CloseHandle(handle)
        self.handles.clear()


class DarwinClock:
    """Unix verification analog: Mach THREAD_BASIC_INFO and kernel thread IDs."""
    source = "python_perf_counter_mach_absolute_time"

    def __init__(self):
        self.api = getattr(ctypes, "PyDLL", ctypes.CDLL)("/usr/lib/libSystem.B.dylib")
        self.task = ctypes.c_uint.in_dll(self.api, "mach_task_self_").value
        self.api.task_threads.argtypes = [ctypes.c_uint, ctypes.POINTER(ctypes.POINTER(ctypes.c_uint)), ctypes.POINTER(ctypes.c_uint)]
        self.api.thread_info.argtypes = [ctypes.c_uint, ctypes.c_int, ctypes.POINTER(ctypes.c_int), ctypes.POINTER(ctypes.c_uint)]
        self.api.mach_port_deallocate.argtypes = [ctypes.c_uint, ctypes.c_uint]
        self.api.vm_deallocate.argtypes = [ctypes.c_uint, ctypes.c_size_t, ctypes.c_size_t]

    def now_ns(self):
        return time.perf_counter_ns()

    def snapshot(self, native_ids):
        ports = ctypes.POINTER(ctypes.c_uint)()
        count = ctypes.c_uint()
        if self.api.task_threads(self.task, ctypes.byref(ports), ctypes.byref(count)):
            raise OSError("task_threads failed")
        result = {}
        try:
            for index in range(count.value):
                port = ports[index]
                try:
                    identity = (ctypes.c_uint64 * 3)()
                    size = ctypes.c_uint(6)
                    if self.api.thread_info(port, 4, ctypes.cast(identity, ctypes.POINTER(ctypes.c_int)), ctypes.byref(size)):
                        continue
                    native_id = identity[0]
                    if native_id not in native_ids:
                        continue
                    basic = (ctypes.c_int * 10)()
                    size = ctypes.c_uint(10)
                    if self.api.thread_info(port, 3, basic, ctypes.byref(size)) == 0:
                        result[native_id] = {"cpu_ns": (basic[0] + basic[2]) * 1_000_000_000 + (basic[1] + basic[3]) * 1000,
                                             "lifetime": str(native_id)}
                finally:
                    self.api.mach_port_deallocate(self.task, port)
        finally:
            self.api.vm_deallocate(self.task, ctypes.cast(ports, ctypes.c_void_p).value, count.value * ctypes.sizeof(ctypes.c_uint))
        return result

    def close(self):
        pass


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
        self.api.pread.argtypes = [ctypes.c_int, ctypes.c_void_p, ctypes.c_size_t, ctypes.c_longlong]
        self.api.pread.restype = ctypes.c_ssize_t
        self.api.close.argtypes = [ctypes.c_int]
        self.api.close.restype = ctypes.c_int
        self.handles = {}
        self.buffer = ctypes.create_string_buffer(4096)

    def now_ns(self):
        return time.perf_counter_ns()

    def snapshot(self, native_ids):
        result = {}
        for tid in set(self.handles) - native_ids:
            self.api.close(self.handles.pop(tid))
        for tid in native_ids:
            try:
                handle = self.handles.get(tid)
                if handle is None:
                    handle = self.api.open(f"/proc/self/task/{tid}/stat".encode(), os.O_RDONLY | os.O_CLOEXEC)
                    if handle < 0:
                        continue
                    self.handles[tid] = handle
                count = self.api.pread(handle, self.buffer, len(self.buffer), 0)
                if count <= 0 or count == len(self.buffer):
                    self.api.close(self.handles.pop(tid))
                    continue
                fields = self.buffer.raw[:count].rsplit(b")", 1)[1].split()
                result[tid] = {"cpu_ns": (int(fields[11]) + int(fields[12])) * 1_000_000_000 // self.frequency,
                               "lifetime": fields[19].decode("ascii")}
            except (OSError, ValueError, IndexError):
                continue
        return result

    def close(self):
        for handle in self.handles.values():
            self.api.close(handle)
        self.handles.clear()


def clock():
    if sys.platform == "win32":
        return WindowsClock()
    if sys.platform == "darwin":
        return DarwinClock()
    if sys.platform.startswith("linux"):
        return LinuxClock()
    raise RuntimeError(f"OS thread CPU counters unavailable on {sys.platform}")
