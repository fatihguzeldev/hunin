"""Exercise a real collector through the installed Hunin CLI; missing support fails."""

import argparse
from contextlib import contextmanager
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import sys
import tempfile

from . import workloads


@contextmanager
def workspace(output):
    if output:
        directory = Path(output).resolve()
        directory.mkdir(parents=True, exist_ok=True)
        try:
            yield str(directory)
        except BaseException as error:
            (directory / "failure.json").write_text(
                json.dumps({"host": platform.platform(), "error": str(error)}, indent=2)
            )
            raise
    else:
        with tempfile.TemporaryDirectory(prefix="hunin-collector-proof-") as directory:
            yield directory


def execute(command, *, cwd=None, env=None, timeout=120):
    result = subprocess.run(
        command, cwd=cwd, env=env, capture_output=True, text=True, timeout=timeout
    )
    if result.returncode:
        raise RuntimeError(
            f"command failed ({result.returncode}): {command}\n{result.stdout}\n{result.stderr}"
        )
    return result.stdout


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True)
    parser.add_argument(
        "--runtime",
        required=True,
        choices=[
            "native",
            "node",
            "bun",
            "deno",
            "cpython",
            "pypy",
            "python-cpu",
            "graalpy",
            "go",
            "go-test",
            "go-trace",
        ],
    )
    parser.add_argument(
        "--executable", help="runtime executable, or rustc for the native control"
    )
    parser.add_argument(
        "--output",
        help="retain workload, store, raw artifacts and evidence in this directory",
    )
    args = parser.parse_args()
    binary = str(Path(args.binary).resolve())
    default = {
        "native": "rustc",
        "cpython": sys.executable,
        "python-cpu": sys.executable,
        "pypy": "pypy3",
        "graalpy": "graalpy",
        "go-test": "go",
        "go-trace": "go",
    }.get(args.runtime, args.runtime)
    executable = shutil.which(args.executable or default)
    with workspace(args.output) as directory:
        work = Path(directory)
        if not executable:
            raise RuntimeError(
                f"required executable missing: {args.executable or default}"
            )
        data = work / "recordings"
        env = dict(os.environ)
        runtime = args.runtime
        control = workloads.prepare(runtime, executable, work, env, execute)
        source, command, expected = (
            control.source,
            control.command,
            control.expected_hotspot,
        )
        result = json.loads(
            execute(
                [
                    binary,
                    "--data-dir",
                    str(data),
                    "run",
                    "--runtime",
                    runtime,
                    "--cwd",
                    str(work),
                    "--timeout",
                    "45",
                    "--",
                    *command,
                ],
                env=env,
            )
        )
        run = result["run"]
        workload = None
        if runtime in ("cpython", "pypy", "python-cpu"):
            workload = json.loads((work / "workload.json").read_text())
            if workload["thread_cpu_seconds"] < 0.6:
                raise RuntimeError(
                    f"Python control did not complete its measured CPU budget: {workload}"
                )
        if run["status"] != "completed" or run["correctness"]["passed"] is not True:
            raise RuntimeError(f"recording did not complete correctly: {result}")
        if runtime == "go-trace":
            if result["summary"]["span_count"] == 0:
                raise RuntimeError("Go trace has no observed runtime ranges")
        else:
            if result["summary"]["sample_count"] == 0:
                raise RuntimeError("collector produced no automatic samples")
            hotspots = json.loads(
                execute(
                    [
                        binary,
                        "--data-dir",
                        str(data),
                        "call",
                        "profiles.hotspots",
                        json.dumps({"run_id": run["id"], "limit": 1000}),
                    ],
                    env=env,
                )
            )
            if not any(expected in row["function"] for row in hotspots["hotspots"]):
                raise RuntimeError(
                    f"known source function missing from profile: {hotspots}"
                )
            if runtime == "go" and not any(
                expected in row["function"]
                and str(row.get("file", ""))
                .replace("\\", "/")
                .endswith("/" + source.name)
                and isinstance(row.get("line"), int)
                and row["line"] > 0
                for row in hotspots["hotspots"]
            ):
                raise RuntimeError(
                    f"ordinary Go executable has no measured hotspot source position: {hotspots}"
                )
        reopened = json.loads(
            execute([binary, "--data-dir", str(data), "inspect", run["id"]], env=env)
        )
        if reopened["summary"] != result["summary"]:
            raise RuntimeError("accepted profile changed after reopening")
        evidence = {
            "host": platform.platform(),
            "architecture": platform.machine(),
            "runtime": runtime,
            "runtime_version": run["source"].get("runtime_version"),
            "run_id": run["id"],
            "summary": result["summary"],
            "diagnostics": run["diagnostics"],
        }
        if workload is not None:
            evidence["workload"] = workload
        if args.output:
            (work / "evidence.json").write_text(json.dumps(evidence, indent=2))
            (work / "run.json").write_text(json.dumps(result, indent=2))
        print(json.dumps(evidence, indent=2))


if __name__ == "__main__":
    main()
