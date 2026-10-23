#!/usr/bin/env python3
"""Exercise an extracted, installed bundle outside its source checkout."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shlex
import signal
import socket
import subprocess
import sys
import tarfile
import time
import urllib.error
import urllib.request
import urllib.parse
import zipfile
from mcp_control import verify_mcp


def adjacent_import(options, installed):
    """Compare parsed file identities, including percent-encoded file URLs."""
    tokens = shlex.split(options)
    imports = [value.split("=", 1)[1] for value in tokens if value.startswith("--import=")]
    imports.extend(tokens[index + 1] for index, value in enumerate(tokens[:-1]) if value == "--import")
    for value in imports:
        uri = urllib.parse.urlsplit(value)
        if uri.scheme == "file":
            path = urllib.request.url2pathname(uri.path)
            if uri.netloc and uri.netloc != "localhost":
                path = f"//{uri.netloc}{path}"
        elif not uri.scheme or Path(value).is_absolute():
            path = value
        else:
            continue
        if Path(path).resolve().is_relative_to(installed.resolve()):
            return True
    return False


def execute(command, work, env):
    result = subprocess.run([str(value) for value in command], cwd=work, env=env, capture_output=True, text=True, timeout=90)
    if result.returncode:
        raise RuntimeError(f"Command failed ({result.returncode}): {command}\n{result.stdout}\n{result.stderr}")
    return result.stdout


def unused_port():
    with socket.socket() as candidate:
        candidate.bind(("127.0.0.1", 0))
        port = candidate.getsockname()[1]
    with socket.socket() as probe:
        if probe.connect_ex(("127.0.0.1", port)) == 0:
            raise RuntimeError(f"Selected loopback port {port} is already in use")
    return port


def verify(archive, output):
    if output.exists() and any(output.iterdir()):
        raise ValueError(f"Preserving existing evidence directory: {output}")
    output.mkdir(parents=True, exist_ok=True)
    try:
        expected = archive.with_name(archive.name + ".sha256").read_text().split()[0]
        with archive.open("rb") as stream:
            actual = hashlib.file_digest(stream, "sha256").hexdigest()
        if expected != actual:
            raise ValueError("Bundle archive checksum mismatch")
        extracted = output / "extracted"
        if archive.suffix == ".zip":
            with zipfile.ZipFile(archive) as bundle:
                bundle.extractall(extracted)
        else:
            with tarfile.open(archive) as bundle:
                bundle.extractall(extracted, filter="data")
        packages = list(extracted.iterdir())
        if len(packages) != 1:
            raise ValueError("Bundle must contain one package directory")
        package = packages[0]
        manifest = json.loads((package / "manifest.json").read_text())
        work = output / "workload"
        work.mkdir()
        environment = dict(os.environ)
        for variable in ("HUNIN_ASSETS_DIR", "HUNIN_UI_DIR", "HUNIN_DATA_DIR"):
            environment.pop(variable, None)
        execute([sys.executable, package / "install.py", "--prefix", output / "installed"], work, environment)
        installed = output / "installed" / f"{manifest['version']}-{manifest['platform']}"
        binary = installed / ("hunin.exe" if os.name == "nt" else "hunin")
        sdk = (installed / "sdk/javascript/index.mjs").as_uri()
        source = work / "main.mjs"
        source.write_text("import {withContext,withSpan,counter,flush} from " + json.dumps(sdk) + ";\n"
                          "function installedBundleHotspot(){const end=Date.now()+300;let total=0;while(Date.now()<end)for(let n=1;n<10000;n++)total+=Math.sqrt(n);return total;}\n"
                          "withContext('installed control',()=>withSpan('installed work',()=>{installedBundleHotspot();counter('completed',1);}));flush();console.log('installed control ok');\n")
        data = output / "recordings"
        result = json.loads(execute([binary, "--data-dir", data, "run", "--name", "installed bundle control", "--runtime", "node", "--cwd", work,
                                     "--timeout", "30", "--expected-stdout-sha256", hashlib.sha256(b"installed control ok\n").hexdigest(), "--", "node", source], work, environment))
        run = result["run"]
        if run["status"] != "completed" or run["correctness"]["passed"] is not True or result["summary"]["sample_count"] == 0 or result["summary"]["span_count"] == 0:
            raise RuntimeError(f"Installed recording failed the profiling/SDK/output control: {result}")
        options = run["source"].get("collector_settings", {}).get("environment", {}).get("NODE_OPTIONS", "")
        if not adjacent_import(options, installed):
            raise RuntimeError("Installed collector unexpectedly used assets outside its bundle")
        def call(operation, params):
            return json.loads(execute([binary, "--data-dir", data, "call", operation, json.dumps(params)], work, environment))
        hotspots = call("profiles.hotspots", {"run_id": run["id"], "limit": 1000})
        if not any("installedBundleHotspot" in row["function"] for row in hotspots["hotspots"]):
            raise RuntimeError("Installed collector did not resolve the known JavaScript hotspot")
        contexts = call("contexts.query", {"run_id": run["id"]})
        if not any(row.get("name") == "installed control" for row in contexts["contexts"]):
            raise RuntimeError("Bundled SDK context did not reach accepted storage")
        source.rename(work / "retained-original.mjs")
        retained = call("source.read", {"run_id": run["id"], "path": str(source), "line": 2, "count": 3})
        if retained.get("source_kind") not in ("recorded_snapshot", "recorded_source_map") or retained.get("identity_verified") is not True or "installedBundleHotspot" not in retained["content"]:
            raise RuntimeError("Installed recording lost the measured source snapshot")
        reopened = json.loads(execute([binary, "--data-dir", data, "inspect", run["id"]], work, environment))
        if reopened["summary"] != result["summary"]:
            raise RuntimeError("Installed recording summary changed after reopening")
        mcp = verify_mcp(binary, data, work, environment, output, run["id"], source, hashlib.sha256(b"installed control ok\n").hexdigest())
        mcp_reopened = json.loads(execute([binary, "--data-dir", data, "inspect", mcp["run_id"]], work, environment))
        if mcp_reopened["summary"] != mcp["summary"]:
            raise RuntimeError("MCP recording or accepted analyzer output changed after reopening through CLI")
        port = unused_port()
        with (output / "server.stdout.txt").open("wb") as stdout, (output / "server.stderr.txt").open("wb") as stderr:
            server = subprocess.Popen([str(binary), "--data-dir", str(data), "ui", "--bind", f"127.0.0.1:{port}"], cwd=work, env=environment, stdout=stdout, stderr=stderr)
            try:
                base = f"http://127.0.0.1:{port}"
                deadline = time.monotonic() + 15
                while True:
                    if server.poll() is not None:
                        raise RuntimeError("Installed UI server exited before becoming ready")
                    try:
                        with urllib.request.urlopen(base + "/health", timeout=1) as response:
                            if json.load(response).get("status") == "ok":
                                break
                    except (urllib.error.URLError, TimeoutError):
                        if time.monotonic() > deadline:
                            raise RuntimeError("Installed UI server readiness timed out")
                        time.sleep(.1)
                with urllib.request.urlopen(base, timeout=5) as response:
                    html = response.read().decode()
                assets = re.findall(r'(?:src|href)="(/assets/[^\"]+)"', html)
                if not assets:
                    raise RuntimeError("Adjacent UI index has no built assets")
                for path in assets:
                    with urllib.request.urlopen(base + path, timeout=5) as response:
                        if not response.read():
                            raise RuntimeError(f"Installed UI asset is empty: {path}")
            finally:
                if server.poll() is None:
                    server.send_signal(signal.SIGINT) if os.name != "nt" else server.terminate()
                    try:
                        server.wait(timeout=10)
                    except subprocess.TimeoutExpired:
                        server.kill()
                        server.wait(timeout=5)
        evidence = {"host": platform.platform(), "architecture": platform.machine(), "platform": manifest["platform"], "bundle_sha256": actual,
                    "run_id": run["id"], "summary": result["summary"], "correctness": run["correctness"], "source_snapshot_verified": True,
                    "adjacent_ui_assets": assets, "adjacent_collectors_and_sdk_verified": True}
        evidence["mcp"] = mcp
        (output / "evidence.json").write_text(json.dumps(evidence, indent=2))
        (output / "run.json").write_text(json.dumps(result, indent=2))
        return evidence
    except BaseException as error:
        (output / "failure.json").write_text(json.dumps({"host": platform.platform(), "error": str(error)}, indent=2))
        raise


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--archive", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    print(json.dumps(verify(args.archive.resolve(), args.output.resolve()), indent=2))
