"""Preserve the GraalPy process identity while its CPUSampler runs."""

import json
import os
from pathlib import Path
import sys
import time
from entry import run_target


if __name__ == "__main__":
    metadata = Path(sys.argv[1])
    target = sys.argv[2:]
    if target[:1] == ["--"]:
        target.pop(0)
    metadata.write_text(json.dumps({"pid": os.getpid(), "interpreter": sys.executable,
                                    "python_version": sys.version,
                                    "epoch_start_us": time.time_ns() // 1000}))
    run_target(target)
