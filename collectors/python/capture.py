"""Run a Python entry point with vmprof, preserving its original recording."""

import json
import os
from pathlib import Path
import sys
import time
from entry import run_target


def main():
    import vmprof

    output = Path(sys.argv[1]).resolve()
    target = sys.argv[2:]
    if target[:1] == ["--"]:
        target.pop(0)
    if not target:
        raise SystemExit("a Python script, -m module or -c code is required")
    metadata = {
        "pid": os.getpid(),
        "interpreter": sys.executable,
        "python_version": sys.version,
        "epoch_start_us": time.time_ns() // 1000,
        "process_cpu_start_us": time.process_time_ns() // 1000,
        "clock_alignment": "unmapped",
        "vmprof_options": {
            "period": 0.00099,
            "lines": sys.implementation.name != "pypy",
            "memory": os.name != "nt",
        },
    }
    with output.open("w+b") as recording:
        # Windows vmprof rejects RSS capture. PyPy ignores line tracing with an
        # stdout warning, which would change the application's checked output.
        vmprof.enable(recording.fileno(), **metadata["vmprof_options"])
        try:
            run_target(target)
        finally:
            vmprof.disable()
            metadata["epoch_end_us"] = time.time_ns() // 1000
            metadata["process_cpu_end_us"] = time.process_time_ns() // 1000
            output.with_suffix(output.suffix + ".meta.json").write_text(
                json.dumps(metadata)
            )


if __name__ == "__main__":
    main()
