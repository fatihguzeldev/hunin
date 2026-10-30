"""Run the pinned symbol-reader oracles against a real controlled PE input."""

import argparse
import json
import os
from pathlib import Path

from . import source as source_inputs, toolchain as rust_toolchain
from .checks import pe_dwarf_check, run_checks


def fixture_from_recording(directory):
    manifest = directory / "run.json"
    result = json.loads(manifest.read_text())
    run = result.get("run", {})
    if (
        run.get("runtime") != "go-test"
        or run.get("status") != "completed"
        or run.get("correctness", {}).get("passed") is not True
    ):
        raise ValueError(
            "PE source checks require a completed, correct Go-test control"
        )
    run_id = run.get("id")
    if (
        not isinstance(run_id, str)
        or not run_id
        or "/" in run_id
        or "\\" in run_id
        or run_id in (".", "..")
    ):
        raise ValueError("Go-test control has no safe recording identity")
    fixture = directory / "recordings/captures" / run_id / "workload.test.exe"
    return fixture, {
        "source": "completed_go_test_control",
        "run_id": run_id,
        "run_manifest_sha256": source_inputs.digest(manifest),
    }


def verify(cache, output, fixture=None, recording=None, toolchain="stable"):
    if (fixture is None) == (recording is None):
        raise ValueError("Select exactly one PE fixture or Go-test recording control")
    if output.exists() and any(output.iterdir()):
        raise ValueError(f"Preserving existing source verification evidence: {output}")
    output.mkdir(parents=True, exist_ok=True)
    context = {
        "schema_version": 1,
        "scope": "pinned source symbol-reader regression checks",
    }
    try:
        if recording is not None:
            fixture, origin = fixture_from_recording(recording)
        else:
            origin = {"source": "explicit_pe_input"}
        fixture = fixture.resolve()
        if not fixture.is_file():
            raise ValueError(f"Required real PE fixture is missing: {fixture}")
        fixture_sha256 = source_inputs.digest(fixture)
        context["fixture"] = {
            "path": str(fixture),
            "sha256": fixture_sha256,
            "size": fixture.stat().st_size,
            "origin": origin,
        }
        source, workspace, identity, patch_sha256 = source_inputs.prepare_inputs(cache)
        compiler, host = rust_toolchain.inspect(toolchain)
        source_sha256 = source_inputs.tree_digest(source)
        context.update(
            {
                "identity": identity,
                "patch_sha256": patch_sha256,
                "source_sha256": source_sha256,
                "rustc": compiler,
                "target": host,
            }
        )
        environment = dict(
            os.environ,
            CARGO_TARGET_DIR=str(workspace / "target"),
            HUNIN_NATIVE_PATCH_SHA256=patch_sha256,
            HUNIN_PE_DWARF_FIXTURE=str(fixture),
        )
        context["checks"] = run_checks(
            source, environment, toolchain, host, [pe_dwarf_check()]
        )
        if (
            source_inputs.digest(fixture) != fixture_sha256
            or source_inputs.tree_digest(source) != source_sha256
        ):
            raise ValueError("Source or fixture bytes changed during regression checks")
        context["limitations"] = (
            "These source oracles check compressed DWARF and profile/symbol-reader identity. "
            "They do not establish ETW capture, OS session cleanup or a separately built sampler's behavior."
        )
        (output / "evidence.json").write_text(json.dumps(context, indent=2) + "\n")
        return context
    except BaseException as error:
        (output / "failure.json").write_text(
            json.dumps({**context, "error": str(error)}, indent=2) + "\n"
        )
        raise


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cache", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--rust-toolchain", default="stable")
    inputs = parser.add_mutually_exclusive_group(required=True)
    inputs.add_argument("--pe-fixture", type=Path)
    inputs.add_argument("--pe-recording", type=Path)
    args = parser.parse_args()
    result = verify(
        args.cache.resolve(),
        args.output.resolve(),
        args.pe_fixture.resolve() if args.pe_fixture else None,
        args.pe_recording.resolve() if args.pe_recording else None,
        args.rust_toolchain,
    )
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
