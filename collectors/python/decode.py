"""Source-checkout compatibility entrypoint for the offline vmprof decoder."""

import json
from pathlib import Path
import runpy
import sys


_implementation = runpy.run_path(
    str(Path(__file__).resolve().parents[2] / "src/importers/vmprof/decode.py"),
    run_name="hunin_vmprof_decoder",
)
decode = _implementation["decode"]


if __name__ == "__main__":
    print(json.dumps(_implementation["decode_with_provenance"](Path(sys.argv[1]))))
