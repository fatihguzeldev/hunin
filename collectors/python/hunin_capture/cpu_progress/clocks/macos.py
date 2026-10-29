import ctypes
import time


class DarwinClock:
    """Unix verification analog: Mach THREAD_BASIC_INFO and kernel thread IDs."""

    source = "python_perf_counter_mach_absolute_time"

    def __init__(self):
        self.api = getattr(ctypes, "PyDLL", ctypes.CDLL)("/usr/lib/libSystem.B.dylib")
        self.task = ctypes.c_uint.in_dll(self.api, "mach_task_self_").value
        self.api.task_threads.argtypes = [
            ctypes.c_uint,
            ctypes.POINTER(ctypes.POINTER(ctypes.c_uint)),
            ctypes.POINTER(ctypes.c_uint),
        ]
        self.api.thread_info.argtypes = [
            ctypes.c_uint,
            ctypes.c_int,
            ctypes.POINTER(ctypes.c_int),
            ctypes.POINTER(ctypes.c_uint),
        ]
        self.api.mach_port_deallocate.argtypes = [ctypes.c_uint, ctypes.c_uint]
        self.api.vm_deallocate.argtypes = [
            ctypes.c_uint,
            ctypes.c_size_t,
            ctypes.c_size_t,
        ]
        self.handles = {}

    def now_ns(self):
        return time.perf_counter_ns()

    def retain(self, native_ids):
        for tid in set(self.handles) - native_ids:
            self.api.mach_port_deallocate(self.task, self.handles.pop(tid))
        if native_ids.issubset(self.handles):
            return
        ports = ctypes.POINTER(ctypes.c_uint)()
        count = ctypes.c_uint()
        if self.api.task_threads(self.task, ctypes.byref(ports), ctypes.byref(count)):
            raise OSError("task_threads failed")
        try:
            for index in range(count.value):
                port = ports[index]
                retained = False
                try:
                    identity = (ctypes.c_uint64 * 3)()
                    size = ctypes.c_uint(6)
                    if self.api.thread_info(
                        port,
                        4,
                        ctypes.cast(identity, ctypes.POINTER(ctypes.c_int)),
                        ctypes.byref(size),
                    ):
                        continue
                    native_id = identity[0]
                    if native_id in native_ids and native_id not in self.handles:
                        self.handles[native_id] = port
                        retained = True
                finally:
                    if not retained:
                        self.api.mach_port_deallocate(self.task, port)
        finally:
            self.api.vm_deallocate(
                self.task,
                ctypes.cast(ports, ctypes.c_void_p).value,
                count.value * ctypes.sizeof(ctypes.c_uint),
            )

    def snapshot(self, native_ids):
        self.retain(set(self.handles) | native_ids)
        result = {}
        for tid in native_ids:
            port = self.handles.get(tid)
            if port is None:
                continue
            basic = (ctypes.c_int * 10)()
            size = ctypes.c_uint(10)
            if self.api.thread_info(port, 3, basic, ctypes.byref(size)) == 0:
                result[tid] = {
                    "cpu_ns": (basic[0] + basic[2]) * 1_000_000_000
                    + (basic[1] + basic[3]) * 1000,
                    "lifetime": str(tid),
                }
        return result

    def close(self):
        for port in self.handles.values():
            self.api.mach_port_deallocate(self.task, port)
        self.handles.clear()
