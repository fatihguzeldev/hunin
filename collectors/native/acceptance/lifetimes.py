#!/usr/bin/env python3
"""Prove mapping generations/gaps/threads and immutable native replay on this host."""

import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess

HERE = Path(__file__).resolve().parents[1]


def execute(command, work, environment):
    result = subprocess.run(
        [str(x) for x in command],
        cwd=work,
        env=environment,
        capture_output=True,
        text=True,
        timeout=90,
    )
    if result.returncode:
        raise RuntimeError(
            f"Control failed ({result.returncode}): {command}\n{result.stdout}\n{result.stderr}"
        )
    return result.stdout


def verify(binary, output, sampler=None):
    if output.exists() and any(output.iterdir()):
        raise ValueError(f"Preserving existing lifetime evidence: {output}")
    output.mkdir(parents=True, exist_ok=True)
    environment = dict(os.environ)
    if sampler:
        environment["HUNIN_SAMPLY"] = str(sampler)
    try:
        source = output / "source"
        shutil.copytree(HERE / "fixtures/lifetimes", source)
        shutil.copytree(source, output / "retained-fixture-source")
        build = output / "build"
        cmake = os.environ.get("HUNIN_TEST_CMAKE", "cmake")
        execute(
            [cmake, "-S", source, "-B", build, "-DCMAKE_BUILD_TYPE=RelWithDebInfo"],
            output,
            environment,
        )
        execute(
            [cmake, "--build", build, "--config", "RelWithDebInfo"], output, environment
        )

        def unique(name):
            paths = list(build.rglob(name))
            if len(paths) != 1:
                raise RuntimeError(f"No unique fixture output: {name}: {paths}")
            return paths[0]

        target = unique("lifetime_target.exe" if os.name == "nt" else "lifetime_target")
        suffix = (
            ".dll"
            if os.name == "nt"
            else ".dylib"
            if os.uname().sysname == "Darwin"
            else ".so"
        )
        prefix = "" if os.name == "nt" else "lib"
        a, b = unique(prefix + "phase_a" + suffix), unique(prefix + "phase_b" + suffix)
        alias = a.with_name(prefix + "phase_alias" + suffix)
        shutil.copy2(a, alias)
        data = output / "recordings"
        recorded = json.loads(
            execute(
                [
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
                    "--",
                    target,
                    a,
                    b,
                    alias,
                ],
                output,
                environment,
            )
        )
        (output / "run.json").write_text(json.dumps(recorded, indent=2))
        run = recorded["run"]
        assert run["status"] == "completed" and run["exit_code"] == 0, run

        def call(operation, params):
            return json.loads(
                execute(
                    [binary, "--data-dir", data, "call", operation, json.dumps(params)],
                    output,
                    environment,
                )
            )

        def rows(kind):
            result = []
            cursor = None
            while True:
                params = {
                    "run_id": run["id"],
                    "kind": kind,
                    "alignment": "all",
                    "limit": 5000,
                }
                if cursor:
                    params["cursor"] = cursor
                page = call("timeline.query", params)
                result.extend(page["records"])
                result.extend(page.get("unaligned_records", []))
                cursor = page.get("next_cursor")
                if not cursor:
                    return result

        spans = rows("span")
        phases = [
            r
            for r in spans
            if r.get("name") == "module.observed_lifetime"
            and "phase_" in r["attributes"].get("module", "")
        ]
        sequential = sorted(
            [r for r in phases if "phase_alias" not in r["attributes"]["module"]],
            key=lambda r: r["timestamp_us"],
        )[:3]
        assert len(sequential) == 3 and [
            "phase_b" in r["attributes"]["module"] for r in sequential
        ] == [False, True, False], phases
        assert len({r["id"] for r in phases}) == len(phases) >= 5, phases
        assert all(r["attributes"]["evidence_complete"] for r in phases), phases
        assert tuple(
            sequential[0]["attributes"]["library"].get(field)
            for field in ("codeId", "breakpadId")
        ) != tuple(
            sequential[1]["attributes"]["library"].get(field)
            for field in ("codeId", "breakpadId")
        ), phases
        gaps = []
        for first, second in zip(sequential, sequential[1:]):
            end = first["timestamp_us"] + first["duration_us"]
            gap = second["timestamp_us"] - end
            assert gap > 40_000, (gap, first, second)
            middle = end + gap // 2
            assert not any(
                r["timestamp_us"] <= middle < r["timestamp_us"] + r["duration_us"]
                for r in phases
            ), phases
            gaps.append(gap)
        simultaneous_alias = next(
            r for r in phases if "phase_alias" in r["attributes"]["module"]
        )
        simultaneous_a = max(
            (r for r in phases if r["attributes"]["module"] == a.name),
            key=lambda r: r["timestamp_us"],
        )
        assert (
            simultaneous_alias["attributes"]["library"]["codeId"]
            == simultaneous_a["attributes"]["library"]["codeId"]
        ), phases
        assert (
            simultaneous_alias["attributes"]["start_avma"]
            != simultaneous_a["attributes"]["start_avma"]
        ), phases
        overlap = min(
            r["timestamp_us"] + r["duration_us"]
            for r in (simultaneous_alias, simultaneous_a)
        ) - max(r["timestamp_us"] for r in (simultaneous_alias, simultaneous_a))
        assert overlap > 100_000, (overlap, phases)
        related = {
            r["to"]
            for r in rows("relation")
            if r.get("relation") == "sampled_in_code_instance"
        }
        assert all(r["id"] in related for r in phases), phases
        threads = [r for r in spans if r.get("name") == "thread.observed_lifetime"]
        assert len(threads) >= 4, threads
        # Remove original code and binaries; replay must use managed profile, sidecar and image blobs.
        cursor = None
        raw = None
        while True:
            page = call(
                "artifacts.list",
                {
                    "run_id": run["id"],
                    "limit": 100,
                    **({"cursor": cursor} if cursor else {}),
                },
            )
            raw = next(
                (r for r in page["artifacts"] if r["name"] == "cpu.samply.json"), None
            )
            cursor = page.get("next_cursor")
            if raw or not cursor:
                break
        assert raw is not None, "Retained raw profile missing from artifact pages"
        shutil.rmtree(source)
        shutil.rmtree(build)
        replay = call(
            "recordings.reanalyze", {"run_id": run["id"], "artifact_id": raw["id"]}
        )
        after = {
            r["id"] for r in rows("span") if r.get("name") == "module.observed_lifetime"
        }
        assert {r["id"] for r in phases}.issubset(after), replay
        proof = {
            "run_id": run["id"],
            "summary": recorded["summary"],
            "module_instances": [
                {
                    "id": r["id"],
                    "module": r["attributes"]["module"],
                    "mapping_instance_id": r["attributes"]["mapping_instance_id"],
                    "basis": r["attributes"]["boundary_basis"],
                }
                for r in phases
            ],
            "observed_gap_us": gaps,
            "simultaneous_overlap_us": overlap,
            "thread_instances": len(threads),
            "offline_replay": replay,
            "limitation": "Snapshot transitions have observation windows; loads entirely between snapshots remain unobserved.",
        }
        (output / "evidence.json").write_text(json.dumps(proof, indent=2))
        return proof
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
