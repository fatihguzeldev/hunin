"""Bounded, isolated descriptor subprocess lifetime and output capture."""

import json
import os
import subprocess
import tempfile
import threading
import time

PROBE_OUTPUT_LIMIT = 64 * 1024


def _probe_output(command, work, environment, timeout=10):
    """Drain both pipes under one byte budget and always reap our child."""
    deadline = time.monotonic() + timeout
    process = subprocess.Popen(
        command,
        cwd=work,
        env=environment,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        bufsize=0,
    )
    outputs = [bytearray(), bytearray()]
    lock = threading.Lock()
    exceeded = threading.Event()
    total = 0

    def drain(stream, output):
        nonlocal total
        try:
            while chunk := stream.read(4096):
                with lock:
                    remaining = max(0, PROBE_OUTPUT_LIMIT - total)
                    output.extend(chunk[:remaining])
                    total += len(chunk)
                    if total > PROBE_OUTPUT_LIMIT:
                        exceeded.set()
                if exceeded.is_set():
                    try:
                        process.kill()
                    except OSError:
                        pass
                    break
        finally:
            stream.close()

    readers = [
        threading.Thread(target=drain, args=(stream, output), daemon=True)
        for stream, output in zip((process.stdout, process.stderr), outputs)
    ]
    for reader in readers:
        reader.start()
    try:
        process.wait(timeout=max(0, deadline - time.monotonic()))
        for reader in readers:
            reader.join(timeout=max(0, deadline - time.monotonic()))
        if exceeded.is_set():
            raise ValueError(
                "Executable descriptor output exceeded the 64 KiB combined stdout/stderr byte limit"
            )
        if any(reader.is_alive() for reader in readers):
            raise subprocess.TimeoutExpired(command, timeout)
        return process.returncode, bytes(outputs[0]), bytes(outputs[1])
    except subprocess.TimeoutExpired as error:
        raise ValueError(
            f"Executable descriptor timed out after {timeout:g} seconds"
        ) from error
    finally:
        if process.poll() is None:
            process.kill()
        process.wait(timeout=1)
        for reader in readers:
            reader.join(timeout=1)


def probe_binary(executable):
    """Ask only the data-free descriptor command from an isolated directory."""
    environment = dict(os.environ)
    for key in ("HUNIN_DATA_DIR", "HUNIN_ASSETS_DIR", "HUNIN_UI_DIR", "HUNIN_SAMPLY"):
        environment.pop(key, None)
    try:
        with tempfile.TemporaryDirectory(prefix="hunin-build-info-") as work:
            status, stdout, stderr = _probe_output(
                [str(executable.resolve()), "build-info"], work, environment
            )
        if status:
            raise ValueError(
                f"Executable descriptor command failed ({status}): {stderr[-2000:].decode('utf-8', errors='replace')}"
            )
        return json.loads(stdout)
    except (OSError, subprocess.TimeoutExpired, ValueError) as error:
        raise ValueError(
            f"Cannot read executable product descriptor; rebuild a matching Hunin binary. {error}"
        ) from error
