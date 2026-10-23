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


@contextmanager
def workspace(output):
    if output:
        directory = Path(output).resolve()
        directory.mkdir(parents=True, exist_ok=True)
        try:
            yield str(directory)
        except BaseException as error:
            (directory / "failure.json").write_text(json.dumps({"host": platform.platform(), "error": str(error)}, indent=2))
            raise
    else:
        with tempfile.TemporaryDirectory(prefix="hunin-collector-proof-") as directory:
            yield directory


def execute(command, *, cwd=None, env=None, timeout=120):
    result = subprocess.run(command, cwd=cwd, env=env, capture_output=True, text=True, timeout=timeout)
    if result.returncode:
        raise RuntimeError(f"command failed ({result.returncode}): {command}\n{result.stdout}\n{result.stderr}")
    return result.stdout


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True)
    parser.add_argument("--runtime", required=True, choices=["native", "node", "bun", "deno", "cpython", "pypy", "python-cpu", "graalpy", "go", "go-test", "go-trace"])
    parser.add_argument("--executable", help="runtime executable, or rustc for the native control")
    parser.add_argument("--output", help="retain workload, store, raw artifacts and evidence in this directory")
    args = parser.parse_args()
    binary = str(Path(args.binary).resolve())
    default = {"native": "rustc", "cpython": sys.executable, "python-cpu": sys.executable, "pypy": "pypy3", "graalpy": "graalpy", "go-test": "go", "go-trace": "go"}.get(args.runtime, args.runtime)
    executable = shutil.which(args.executable or default)
    with workspace(args.output) as directory:
        work = Path(directory)
        if not executable:
            raise RuntimeError(f"required executable missing: {args.executable or default}")
        data = work / "recordings"
        env = dict(os.environ)
        runtime = args.runtime
        expected = "huninSmokeHotspot"
        if runtime == "native":
            source = work / "cpu.rs"
            source.write_text("#[inline(never)] fn huninSmokeHotspot()->u64 {let mut x=1u64;for n in 0..250_000_000u64{x=(x^n).wrapping_mul(6364136223846793005).wrapping_add(1);}std::hint::black_box(x)} fn main(){println!(\"{}\",huninSmokeHotspot());}")
            target = work / ("cpu.exe" if os.name == "nt" else "cpu")
            execute([executable, "-C", "opt-level=1", "-C", "debuginfo=2", "-C", "force-frame-pointers=yes", str(source), "-o", str(target)])
            command = [str(target)]
        elif runtime in ("cpython", "pypy", "python-cpu", "graalpy"):
            source = work / "cpu.py"
            source.write_text("def huninSmokeHotspot():\n    x=1\n    for n in range(10_000_000):\n        x=((x^n)*1664525+1013904223)&0xffffffff\n    return x\nprint(huninSmokeHotspot())\n")
            command = [executable, str(source)]
            if runtime != "graalpy":
                env["HUNIN_VMPROF_PYTHON"] = executable
        elif runtime in ("go", "go-test", "go-trace"):
            expected = "huninSmokeHotspot"
            (work / "go.mod").write_text("module hunin-collector-proof\n\ngo 1.22\n")
            body = "//go:noinline\nfunc huninSmokeHotspot()uint64{x:=uint64(1);for n:=uint64(0);n<250000000;n++{x=(x^n)*6364136223846793005+1};return x}\n"
            if runtime == "go":
                source = work / "main.go"
                source.write_text('package main\nimport "fmt"\n' + body + 'func main(){fmt.Println(huninSmokeHotspot())}\n')
                target = work / ("cpu.exe" if os.name == "nt" else "cpu")
                execute([executable, "build", "-o", str(target), str(source)], cwd=work)
                command = [str(target)]
            else:
                source = work / "cpu_test.go"
                source.write_text('package control\nimport "testing"\n' + body + 'var Result uint64\nfunc TestCPU(t *testing.T){Result=huninSmokeHotspot();if Result!=12795955527751812353{t.Fatal(Result)}}\n')
                command = [executable, "test", "-count=1", "."]
                env["HUNIN_GO"] = executable
        else:
            source = work / "cpu.mjs"
            source.write_text("function huninSmokeHotspot(){const end=Date.now()+400;let x=0;while(Date.now()<end)for(let n=1;n<10000;n++)x+=Math.sqrt(n);return x;}console.log(huninSmokeHotspot());\n")
            command = [executable, "run", str(source)] if runtime == "deno" else [executable, str(source)]
        result = json.loads(execute([binary, "--data-dir", str(data), "run", "--runtime", runtime,
                                     "--cwd", str(work), "--timeout", "45", "--", *command], env=env))
        run = result["run"]
        if run["status"] != "completed" or run["correctness"]["passed"] is not True:
            raise RuntimeError(f"recording did not complete correctly: {result}")
        if runtime == "go-trace":
            if result["summary"]["span_count"] == 0:
                raise RuntimeError("Go trace has no observed runtime ranges")
        else:
            if result["summary"]["sample_count"] == 0:
                raise RuntimeError("collector produced no automatic samples")
            hotspots = json.loads(execute([binary, "--data-dir", str(data), "call", "profiles.hotspots", json.dumps({"run_id": run["id"], "limit": 1000})], env=env))
            if not any(expected in row["function"] for row in hotspots["hotspots"]):
                raise RuntimeError(f"known source function missing from profile: {hotspots}")
        reopened = json.loads(execute([binary, "--data-dir", str(data), "inspect", run["id"]], env=env))
        if reopened["summary"] != result["summary"]:
            raise RuntimeError("accepted profile changed after reopening")
        evidence={"host": platform.platform(), "architecture": platform.machine(),
                          "runtime": runtime, "runtime_version": run["source"].get("runtime_version"),
                          "run_id":run["id"],"summary": result["summary"], "diagnostics": run["diagnostics"]}
        if args.output:
            (work / "evidence.json").write_text(json.dumps(evidence,indent=2))
            (work / "run.json").write_text(json.dumps(result,indent=2))
        print(json.dumps(evidence, indent=2))


if __name__ == "__main__":
    main()
