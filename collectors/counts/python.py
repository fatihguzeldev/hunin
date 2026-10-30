"""Run an unchanged Python entry point with bounded native monitoring callbacks."""

import atexit
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import sys
import time


ROOT = Path(__file__).resolve().parent


def write_manifest(path, value):
    temporary = path.with_suffix(".pending")
    temporary.write_text(json.dumps(value, ensure_ascii=True), encoding="utf-8")
    temporary.replace(path)


def main():
    config_path = Path(sys.argv[1])
    config = json.loads(config_path.read_text())
    target = sys.argv[3:]
    extension = Path(config["extension"])
    spec = importlib.util.spec_from_file_location("_hunin_counts", extension)
    native = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(native)
    native.configure(config["max_functions"], config["functions"])
    sys.path.insert(0, str(ROOT.parent / "python"))
    from hunin_capture.target import run_target

    tool = None
    for candidate in reversed(range(6)):
        try:
            sys.monitoring.use_tool_id(candidate, "hunin-function-counts")
            tool = candidate
            break
        except ValueError:
            continue
    if tool is None:
        raise RuntimeError("no free sys.monitoring tool ID for function counting")
    started = time.time_ns() // 1000
    producer = f"python-counts-{os.getpid()}"
    completion = Path(config["completion"])
    output = Path(config["output"])
    manifest = {
        "schema_version": 1,
        "run_id": config["run_id"],
        "method": "cpython-monitoring-native",
        "status": "running",
        "scope": "main_interpreter_python_code_during_monitored_window",
        "collection_scope": "all_then_filter",
        "started_at_us": started,
        "loss": "unknown_until_finalization",
    }
    write_manifest(completion, manifest)
    target_ok = False

    def flush():
        sys.monitoring.set_events(tool, 0)
        sys.monitoring.free_tool_id(tool)
        try:
            rows, dropped, overflow = native.snapshot()
            timestamp = time.time_ns() // 1000
            digest = hashlib.sha256()
            total = 0
            records = 0
            truncated = False
            with output.open("xb") as stream:
                for index, (code, starts, resumes) in enumerate(rows):
                    function = {
                        "name": code.co_qualname,
                        "file": code.co_filename,
                        "line": code.co_firstlineno,
                        "code_id": f"{producer}:{index}",
                    }
                    for semantics, value in (("starts", starts), ("resumes", resumes)):
                        if not value:
                            continue
                        record = {
                            "schema_version": 1,
                            "kind": "metric",
                            "id": f"{producer}:{index}:{semantics}",
                            "run_id": config["run_id"],
                            "producer_id": producer,
                            "pid": os.getpid(),
                            "tid": "process",
                            "timestamp_us": timestamp,
                            "basis": "observed",
                            "name": "function.calls"
                            if semantics == "starts"
                            else "function.resumes",
                            "value": value,
                            "unit": "count",
                            "scope": "process",
                            "attributes": {
                                "function": function,
                                "count_semantics": semantics,
                                "method": manifest["method"],
                                "collection_scope": "all_then_filter",
                                "clock": "unix_epoch",
                                "clock_alignment": "direct_epoch",
                                "clock_source": "time.time_ns",
                                "window_started_at_us": started,
                                "window_finished_at_us": timestamp,
                                "counter_temporality": "delta",
                                "loss": "none_observed"
                                if not dropped and not overflow
                                else "incomplete",
                            },
                        }
                        encoded = (
                            json.dumps(record, ensure_ascii=True, separators=(",", ":"))
                            + "\n"
                        ).encode()
                        if total + len(encoded) > config["max_output_bytes"]:
                            truncated = True
                            break
                        stream.write(encoded)
                        digest.update(encoded)
                        total += len(encoded)
                        records += 1
                    if truncated:
                        break
            manifest.update(
                status="complete"
                if target_ok and not dropped and not overflow and not truncated
                else "incomplete",
                finished_at_us=timestamp,
                records=records,
                bytes=total,
                sha256=digest.hexdigest(),
                dropped_events=dropped,
                counter_overflow=bool(overflow),
                output_truncated=truncated,
                loss="none_observed"
                if not dropped and not overflow and not truncated
                else "incomplete",
                limitations=[
                    "Python code starts and resumes only; C builtins, child processes and subinterpreters are outside this inventory",
                    "All Python-code callbacks execute; exact selectors filter counter admission",
                    "Code objects are retained in a bounded table; their existing constants may retain additional target-owned memory",
                    "Target removal or replacement of the monitoring tool is not detected",
                ],
            )
            write_manifest(completion, manifest)
        except BaseException as error:
            manifest.update(status="incomplete", error=str(error)[:4096])
            write_manifest(completion, manifest)

    # Registered before target handlers: their atexit work runs before this flush.
    atexit.register(flush)
    sys.monitoring.register_callback(
        tool, sys.monitoring.events.PY_START, native.on_start
    )
    sys.monitoring.register_callback(
        tool, sys.monitoring.events.PY_RESUME, native.on_resume
    )
    sys.monitoring.set_events(
        tool, sys.monitoring.events.PY_START | sys.monitoring.events.PY_RESUME
    )
    try:
        run_target(target)
        target_ok = True
    except SystemExit as error:
        target_ok = error.code is None or error.code == 0
        raise


if __name__ == "__main__":
    main()
