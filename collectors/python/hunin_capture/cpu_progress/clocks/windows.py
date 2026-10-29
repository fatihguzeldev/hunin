import ctypes
import time


class WindowsClock:
    def __init__(self):
        from ctypes import wintypes

        self.api = getattr(ctypes, "PyDLL", ctypes.CDLL)(
            "kernel32", use_last_error=True
        )
        self.api.OpenThread.argtypes = [wintypes.DWORD, wintypes.BOOL, wintypes.DWORD]
        self.api.OpenThread.restype = wintypes.HANDLE
        self.api.CloseHandle.argtypes = [wintypes.HANDLE]
        self.api.GetThreadTimes.argtypes = [wintypes.HANDLE] + [
            ctypes.POINTER(wintypes.FILETIME)
        ] * 4
        self.api.GetThreadTimes.restype = wintypes.BOOL
        self.source = (
            "python_perf_counter:" + time.get_clock_info("perf_counter").implementation
        )
        self.handles = {}
        self.filetime = wintypes.FILETIME
        self.unmapped_ended = []

    def now_ns(self):
        # Both supported interpreters implement this with QPC on Windows.
        # Calling their builtin avoids an extra PyPy ctypes GIL handoff.
        return time.perf_counter_ns()

    def retain(self, native_ids):
        for tid in set(self.handles) - native_ids:
            self.api.CloseHandle(self.handles.pop(tid))
        for tid in native_ids:
            if tid not in self.handles:
                handle = self.api.OpenThread(
                    0x0800, False, tid
                )  # THREAD_QUERY_LIMITED_INFORMATION
                if handle:
                    self.handles[tid] = handle

    def snapshot(self, native_ids):
        self.retain(set(self.handles) | native_ids)
        result = {}
        for tid in native_ids:
            handle = self.handles.get(tid)
            if handle is None:
                continue
            created, ended, kernel, user = (self.filetime() for _ in range(4))
            if self.api.GetThreadTimes(
                handle,
                ctypes.byref(created),
                ctypes.byref(ended),
                ctypes.byref(kernel),
                ctypes.byref(user),
            ):

                def ticks(item):
                    return (item.dwHighDateTime << 32) | item.dwLowDateTime

                if ticks(ended) > 0:
                    # A cached handle can outlive its thread. The numeric ID
                    # may already name another Python thread; retain its final
                    # counter without associating that thread's current frame.
                    self.unmapped_ended.append(
                        {
                            "native_tid": tid,
                            "thread_lifetime": str(ticks(created)),
                            "cpu_counter_ns": (ticks(kernel) + ticks(user)) * 100,
                            "ended_filetime_100ns": ticks(ended),
                        }
                    )
                    self.api.CloseHandle(self.handles.pop(tid))
                    continue
                result[tid] = {
                    "cpu_ns": (ticks(kernel) + ticks(user)) * 100,
                    "lifetime": str(ticks(created)),
                }
        return result

    def take_ended_counters(self):
        counters, self.unmapped_ended = self.unmapped_ended, []
        return counters

    def close(self):
        for handle in self.handles.values():
            self.api.CloseHandle(handle)
        self.handles.clear()
