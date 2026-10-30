"""Prepare known target workloads independently of recording acceptance."""

from dataclasses import dataclass
import os
from pathlib import Path


@dataclass(frozen=True)
class Workload:
    source: Path
    command: list[str]
    expected_hotspot: str


def prepare(runtime, executable, work, env, execute):
    expected = "huninSmokeHotspot"
    if runtime == "native":
        source = work / "cpu.rs"
        source.write_text(
            '#[inline(never)] fn huninSmokeHotspot()->u64 {let mut x=1u64;for n in 0..250_000_000u64{x=(x^n).wrapping_mul(6364136223846793005).wrapping_add(1);}std::hint::black_box(x)} fn main(){println!("{}",huninSmokeHotspot());}'
        )
        target = work / ("cpu.exe" if os.name == "nt" else "cpu")
        execute(
            [
                executable,
                "-C",
                "opt-level=1",
                "-C",
                "debuginfo=2",
                "-C",
                "force-frame-pointers=yes",
                str(source),
                "-o",
                str(target),
            ]
        )
        command = [str(target)]
    elif runtime in ("cpython", "pypy", "python-cpu", "graalpy"):
        source = work / "cpu.py"
        source.write_text(
            "def huninSmokeHotspot():\n    x=1\n    for n in range(10_000_000):\n        x=((x^n)*1664525+1013904223)&0xffffffff\n    return x\nprint(huninSmokeHotspot())\n"
            if runtime == "graalpy"
            else "import json,time\nfrom pathlib import Path\ndef huninSmokeHotspot():\n    cpu_start=time.thread_time()\n    wall_start=time.perf_counter()\n    x=1\n    iterations=0\n    while time.thread_time()-cpu_start<0.6:\n        for n in range(10000):\n            x=((x^n)*1664525+1013904223)&0xffffffff\n        iterations+=10000\n    window={'function':'huninSmokeHotspot','thread_cpu_seconds':time.thread_time()-cpu_start,'wall_seconds':time.perf_counter()-wall_start,'iterations':iterations}\n    return x,window\nvalue,window=huninSmokeHotspot()\nPath(__file__).with_name('workload.json').write_text(json.dumps(window))\nprint(value)\n"
        )
        command = [executable, str(source)]
        if runtime != "graalpy":
            env["HUNIN_VMPROF_PYTHON"] = executable
    elif runtime in ("go", "go-test", "go-trace"):
        expected = "huninSmokeHotspot"
        (work / "go.mod").write_text("module hunin-collector-proof\n\ngo 1.22\n")
        body = "//go:noinline\nfunc huninSmokeHotspot()uint64{x:=uint64(1);for n:=uint64(0);n<250000000;n++{x=(x^n)*6364136223846793005+1};return x}\n"
        if runtime == "go":
            source = work / "main.go"
            source.write_text(
                'package main\nimport "fmt"\n'
                + body
                + "func main(){fmt.Println(huninSmokeHotspot())}\n"
            )
            target = work / ("cpu.exe" if os.name == "nt" else "cpu")
            execute([executable, "build", "-o", str(target), str(source)], cwd=work)
            command = [str(target)]
        else:
            source = work / "cpu_test.go"
            source.write_text(
                'package control\nimport "testing"\n'
                + body
                + "var Result uint64\nfunc TestCPU(t *testing.T){Result=huninSmokeHotspot();if Result!=12795955527751812353{t.Fatal(Result)}}\n"
            )
            command = [executable, "test", "-count=1", "."]
            env["HUNIN_GO"] = executable
    else:
        source = work / "cpu.mjs"
        source.write_text(
            "function huninSmokeHotspot(){const end=Date.now()+400;let x=0;while(Date.now()<end)for(let n=1;n<10000;n++)x+=Math.sqrt(n);return x;}console.log(huninSmokeHotspot());\n"
        )
        command = (
            [executable, "run", str(source)]
            if runtime == "deno"
            else [executable, str(source)]
        )
    return Workload(source, command, expected)
