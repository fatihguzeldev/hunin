import cProfile
import importlib.util
import json
import os
from pathlib import Path
import platform
import sys
import time
import traceback


def code_key(code):
    if isinstance(code, str):
        return ("builtin", code)
    return ("python", code.co_filename, code.co_firstlineno, code.co_name)


def export(profiler):
    entries = profiler.getstats()
    identities = {}
    object_identities = {}
    for index, entry in enumerate(entries):
        identities.setdefault(code_key(entry.code), []).append(index)
        object_identities.setdefault(id(entry.code), []).append(index)
    functions = []
    resolution_counts = {}
    for index, entry in enumerate(entries):
        code = entry.code
        function = {
            "id": index, "name": code if isinstance(code, str) else code.co_name,
            "file": None if isinstance(code, str) else code.co_filename,
            "line": None if isinstance(code, str) else code.co_firstlineno,
            "builtin": isinstance(code, str), "calls": entry.callcount,
            "recursive_calls": entry.reccallcount,
            "total_seconds": entry.totaltime, "self_seconds": entry.inlinetime,
            "callees": [],
        }
        for call in entry.calls or []:
            targets = object_identities.get(id(call.code), [])
            resolution = "object_identity"
            if len(targets) != 1 and isinstance(call.code, str):
                targets = identities.get(code_key(call.code), [])
                resolution = "unique_builtin_name" if len(targets) == 1 else "missing_aggregate" if not targets else "ambiguous_builtin_name"
            elif len(targets) != 1:
                resolution = "missing_code_object" if not targets else "ambiguous_code_object"
            resolution_counts[resolution] = resolution_counts.get(resolution, 0) + 1
            function["callees"].append({
                "callee": targets[0] if len(targets) == 1 else None,
                "callee_resolution": resolution,
                "callee_code": list(code_key(call.code)),
                "calls": call.callcount, "recursive_calls": call.reccallcount,
                "total_seconds": call.totaltime, "self_seconds": call.inlinetime,
            })
        functions.append(function)
    return {"runtime": platform.python_implementation().lower(), "version": platform.python_version(),
            "vm_version": list(sys.pypy_version_info) if hasattr(sys, "pypy_version_info") else list(sys.version_info),
            "pid": os.getpid(), "functions": functions,
            "callee_resolution_counts": resolution_counts,
            "thread_coverage": "bootstrap thread only", "timer": "cProfile default timer",
            "units": {"total_seconds": "profiler elapsed seconds", "self_seconds": "profiler elapsed seconds"}}


def main():
    control_token = os.environ.get("HUNIN_CONTROL_TOKEN")
    finalize_gate = os.environ.get("HUNIN_FINALIZE_GATE") == "1"
    control_write, control_flush = sys.stdout.write, sys.stdout.flush
    control_read = sys.stdin.readline

    def control_line(name):
        control_write(name + ("" if control_token is None else " " + control_token) + "\n")
        control_flush()

    workload, profile_path, report_path = [Path(p) for p in sys.argv[1:4]]
    if not all(p.is_absolute() for p in [workload, profile_path, report_path]):
        raise ValueError("helper paths must be absolute")
    profiler = cProfile.Profile()
    start_before_ns = time.monotonic_ns()
    profiler.enable()
    start_after_ns = time.monotonic_ns()
    control_line("HUNIN_READY")
    command = control_read().rstrip("\r\n")
    output = None
    application_error = None
    outcome = "stopped_before_work"
    work_start_ns = None
    work_end_ns = None
    try:
        if command == "HUNIN_GO":
            work_start_ns = time.monotonic_ns()
            spec = importlib.util.spec_from_file_location("hunin_prepared_workload", workload)
            module = importlib.util.module_from_spec(spec)
            spec.loader.exec_module(module)
            output = module.run()
            outcome = "completed"
        elif command not in ["HUNIN_STOP", ""]:
            raise ValueError("invalid prepared-work gate command")
    except BaseException:
        outcome = "application_failed"
        application_error = traceback.format_exc()
    finally:
        if work_start_ns is not None:
            work_end_ns = time.monotonic_ns()
        profiler.disable()
    stop_ns = time.monotonic_ns()
    with profile_path.open("x") as stream:
        json.dump(export(profiler), stream, allow_nan=False)
        stream.flush()
        os.fsync(stream.fileno())
    with report_path.open("x") as stream:
        json.dump({"outcome": outcome, "output": output, "application_error": application_error,
                   "helper_clock": "time.monotonic_ns", "start_before_ns": start_before_ns,
                   "start_after_ns": start_after_ns, "stop_ns": stop_ns,
                   "work_start_ns": work_start_ns, "work_end_ns": work_end_ns}, stream, allow_nan=False)
        stream.flush()
        os.fsync(stream.fileno())
    if finalize_gate:
        control_line("HUNIN_WORK_COMPLETE")
        finish = control_read().rstrip("\r\n")
        if finish not in ["HUNIN_EXIT", "HUNIN_STOP", ""]:
            raise ValueError("invalid finalize gate command")
    return 1 if application_error else 0


if __name__ == "__main__":
    sys.exit(main())
