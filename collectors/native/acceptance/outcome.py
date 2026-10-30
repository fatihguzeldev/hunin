#!/usr/bin/env python3
"""Exercise native target failure and a distinct pre-launch collector failure."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess

HERE = Path(__file__).resolve().parents[1]


def execute(command, work, environment=None, accepted_codes=(0,)):
    result = subprocess.run(
        [str(x) for x in command],
        cwd=work,
        env=environment,
        capture_output=True,
        text=True,
        timeout=90,
    )
    if result.returncode not in accepted_codes:
        raise RuntimeError(
            f"Control command failed ({result.returncode}): {command}\n{result.stdout}\n{result.stderr}"
        )
    return result.stdout


def verify(binary, output, sampler=None):
    if output.exists() and any(output.iterdir()):
        raise ValueError(f"Preserving existing outcome evidence: {output}")
    output.mkdir(parents=True, exist_ok=True)
    try:
        source = output / "source"
        source.mkdir()
        for name in ("target_outcome.c", "failed_collector.c"):
            shutil.copy2(HERE / "fixtures" / name, source / name)
        (source / "CMakeLists.txt").write_text(
            "cmake_minimum_required(VERSION 3.16)\nproject(native_outcome C)\nadd_executable(negative target_outcome.c)\nadd_executable(failed_collector failed_collector.c)\n"
        )
        build = output / "build"
        cmake = os.environ.get("HUNIN_TEST_CMAKE", "cmake")
        execute(
            [cmake, "-S", source, "-B", build, "-DCMAKE_BUILD_TYPE=RelWithDebInfo"],
            output,
        )
        execute([cmake, "--build", build, "--config", "RelWithDebInfo"], output)

        def executable(name):
            candidates = list(build.rglob(name + (".exe" if os.name == "nt" else "")))
            if len(candidates) != 1:
                raise RuntimeError(f"No unique control executable: {candidates}")
            return candidates[0]

        target = executable("negative")
        environment = dict(os.environ)
        if sampler:
            environment["HUNIN_SAMPLY"] = str(sampler)
        data = output / "recordings"
        expected = (
            b"negative native target\r\n"
            if os.name == "nt"
            else b"negative native target\n"
        )
        request = [
            binary,
            "--data-dir",
            data,
            "run",
            "--runtime",
            "native",
            "--cwd",
            source,
            "--timeout",
            "30",
            "--expected-stdout-sha256",
            hashlib.sha256(expected).hexdigest(),
            "--",
            target,
        ]
        result = json.loads(
            execute(request, output, environment, accepted_codes=(0, 1))
        )
        (output / "negative.json").write_text(json.dumps(result, indent=2))
        run = result["run"]
        assert run["exit_code"] == 2, run
        assert run["source"]["target_outcome"]["state"] == "exited", run
        assert run["source"]["target_outcome"]["exit_code"] == 2, run
        assert run["source"]["collector_outcome"]["exit_code"] == 2, run
        assert run["correctness"]["checks"][0]["passed"] is False, run
        assert run["correctness"]["checks"][1]["passed"] is True, run
        assert (
            run["status"] == "failed"
            and run["source"]["collection_status"] == "completed"
        ), run
        assert result["summary"]["sample_count"] > 0, result
        events = json.loads(
            execute(
                [
                    binary,
                    "--data-dir",
                    data,
                    "call",
                    "timeline.query",
                    json.dumps(
                        {
                            "run_id": run["id"],
                            "kind": "event",
                            "alignment": "all",
                            "limit": 1000,
                        }
                    ),
                ],
                output,
                environment,
            )
        )
        normalized = [
            r
            for r in [*events["records"], *events.get("unaligned_records", [])]
            if r.get("name") == "process.wait_outcome"
        ]
        assert (
            len(normalized) == 1
            and normalized[0]["attributes"]["target_outcome"]["exit_code"] == 2
        ), normalized
        signal_run = None
        if os.name != "nt":
            signaled = json.loads(
                execute(
                    [*request, "signal"], output, environment, accepted_codes=(0, 1)
                )
            )
            (output / "signaled-target.json").write_text(json.dumps(signaled, indent=2))
            signal_run = signaled["run"]["id"]
            assert signaled["run"]["exit_code"] is None, signaled
            assert signaled["run"]["source"]["target_outcome"]["state"] == "signaled", (
                signaled
            )
            assert (
                signaled["run"]["source"]["target_outcome"]["signal"] == signal.SIGTERM
            ), signaled
            assert signaled["run"]["source"]["collection_status"] == "completed", (
                signaled
            )
        environment["HUNIN_SAMPLY"] = str(executable("failed_collector"))
        failed = json.loads(
            execute(request, output, environment, accepted_codes=(0, 1))
        )
        (output / "collector-failure.json").write_text(json.dumps(failed, indent=2))
        run = failed["run"]
        assert run["exit_code"] is None, run
        assert run["source"]["target_outcome"]["state"] == "unknown", run
        assert run["source"]["collector_outcome"]["exit_code"] == 73, run
        assert run["correctness"]["checks"][0]["actual"] is None, run
        assert (
            run["status"] == "failed" and run["source"]["collection_status"] == "failed"
        ), run
        evidence = {
            "negative_target_run": result["run"]["id"],
            "target_exit_code": 2,
            "signaled_target_run": signal_run,
            "collector_failure_run": run["id"],
            "collector_exit_code": 73,
            "unknown_target_exit": run["exit_code"],
            "sample_count": result["summary"]["sample_count"],
        }
        (output / "evidence.json").write_text(json.dumps(evidence, indent=2))
        return evidence
    except BaseException as error:
        (output / "failure.json").write_text(
            json.dumps({"error": str(error)}, indent=2)
        )
        raise


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--sampler", type=Path)
    args = parser.parse_args()
    print(
        json.dumps(
            verify(
                args.binary.resolve(),
                args.output.resolve(),
                args.sampler.resolve() if args.sampler else None,
            ),
            indent=2,
        )
    )


if __name__ == "__main__":
    main()
