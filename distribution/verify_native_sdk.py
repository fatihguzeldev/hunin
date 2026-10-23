#!/usr/bin/env python3
"""Record one native call with both automatic CPU samples and the installed C SDK."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import subprocess


def execute(command, work):
    result = subprocess.run([str(value) for value in command], cwd=work, capture_output=True, text=True, timeout=90)
    if result.returncode:
        raise RuntimeError(f"Command failed ({result.returncode}): {command}\n{result.stdout}\n{result.stderr}")
    return result.stdout


def verify(installed, output):
    if output.exists() and any(output.iterdir()):
        raise ValueError(f"Preserving existing native evidence directory: {output}")
    output.mkdir(parents=True, exist_ok=True)
    try:
        manifest = json.loads((installed / "manifest.json").read_text())
        binary = installed / ("hunin.exe" if os.name == "nt" else "hunin")
        source = output / "source"
        source.mkdir()
        (source / "main.c").write_text(r'''#include "hunin.h"
#include <stdint.h>
#include <stdio.h>
#if defined(_MSC_VER)
__declspec(noinline)
#else
__attribute__((noinline))
#endif
uint64_t installedNativeHotspot(void) {
  volatile uint64_t value = 1;
  for (uint64_t index = 0; index < 150000000; index++) value = (value ^ index) * UINT64_C(6364136223846793005) + 1;
  return value;
}
int main(void) {
  hunin_context_t context = hunin_context_new("installed native context");
  hunin_span_t span;
  hunin_span_begin(&span, "installed native work", &context);
  volatile uint64_t result = installedNativeHotspot();
  (void)result;
  hunin_context_t active = hunin_span_context(&span);
  hunin_counter(&active, "completed", 1, "count");
  hunin_span_end(&span);
  if (hunin_flush()) return 2;
  printf("native SDK control ok\n");
  return 0;
}
''')
        sdk = (installed / "sdk/c").as_posix().replace('"', '\\"')
        (source / "CMakeLists.txt").write_text('cmake_minimum_required(VERSION 3.16)\nproject(hunin_installed_native_control C)\n'
                                              f'add_subdirectory("{sdk}" hunin-sdk)\nadd_executable(native-control main.c)\n'
                                              'target_link_libraries(native-control PRIVATE hunin_sdk)\nset_property(TARGET native-control PROPERTY C_STANDARD 11)\n')
        build = output / "build"
        cmake = os.environ.get("HUNIN_TEST_CMAKE", "cmake")
        execute([cmake, "-S", source, "-B", build, "-DCMAKE_BUILD_TYPE=RelWithDebInfo"], output)
        execute([cmake, "--build", build, "--config", "RelWithDebInfo"], output)
        candidates = list(build.rglob("native-control.exe" if os.name == "nt" else "native-control"))
        if len(candidates) != 1:
            raise RuntimeError(f"No unique installed SDK control executable: {candidates}")
        target = candidates[0]
        data = output / "recordings"
        expected = b"native SDK control ok\r\n" if os.name == "nt" else b"native SDK control ok\n"
        recording = json.loads(execute([binary, "--data-dir", data, "run", "--runtime", "native", "--cwd", source, "--timeout", "30",
                                        "--expected-stdout-sha256", hashlib.sha256(expected).hexdigest(), "--", target], output))
        run = recording["run"]
        if run["status"] != "completed" or run["correctness"]["passed"] is not True or recording["summary"]["sample_count"] == 0:
            raise RuntimeError(f"Installed native control did not complete with an automatic profile: {recording}")
        def call(operation, params):
            return json.loads(execute([binary, "--data-dir", data, "call", operation, json.dumps(params)], output))
        hotspots = call("profiles.hotspots", {"run_id": run["id"], "limit": 1000})
        hot = next((row for row in hotspots["hotspots"] if "installedNativeHotspot" in row["function"]), None)
        if not hot:
            raise RuntimeError("Installed native profile does not contain its known C function")
        contexts = call("contexts.query", {"run_id": run["id"]})["contexts"]
        context = next((row for row in contexts if row.get("name") == "installed native context"), None)
        spans = call("timeline.query", {"run_id": run["id"], "kind": "span", "alignment": "all"})
        span = next((row for row in [*spans["records"], *spans.get("unaligned_records", [])] if row.get("name") == "installed native work"), None)
        if not context or not span or span.get("context_id") != context["id"]:
            raise RuntimeError("Native SDK context and span did not join the automatic recording")
        evidence = call("evidence.get", {"run_id": run["id"], "ids": [context["id"], span["id"]]})
        if len(evidence["records"]) != 2:
            raise RuntimeError("Native context/span evidence did not survive query resolution")
        artifacts = call("artifacts.list", {"run_id": run["id"]})["artifacts"]
        if not any(row["format"] == "hunin-jsonl" for row in artifacts):
            raise RuntimeError("Native SDK raw evidence was not retained")
        reopened = json.loads(execute([binary, "--data-dir", data, "inspect", run["id"]], output))
        if reopened["summary"] != recording["summary"]:
            raise RuntimeError("Combined native/SDK recording changed after reopening")
        result = {"host": platform.platform(), "architecture": platform.machine(), "platform": manifest["platform"], "run_id": run["id"],
                  "summary": recording["summary"], "hotspot": hot, "context_id": context["id"], "span_id": span["id"],
                  "known_control": "The retained C source calls the sampled native function inside its explicit SDK span.",
                  "limitation": "This control does not infer general per-context CPU ownership or map unrelated thread-ID namespaces."}
        (output / "evidence.json").write_text(json.dumps(result, indent=2))
        (output / "run.json").write_text(json.dumps(recording, indent=2))
        return result
    except BaseException as error:
        (output / "failure.json").write_text(json.dumps({"host": platform.platform(), "error": str(error)}, indent=2))
        raise


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--installed", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    print(json.dumps(verify(args.installed.resolve(), args.output.resolve()), indent=2))
