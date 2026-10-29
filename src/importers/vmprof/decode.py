"""Decode vmprof 0.6 recordings without inventing wall-clock sample times."""

import hashlib
import importlib.metadata
import importlib.util
import json
import locale
from pathlib import Path
import sys
import tempfile


def decode(path):
    from vmprof.reader import DEFAULT_MAX_SAMPLE_GAP, LogReader, LogReaderState, gunzip

    times = []

    class TimedReader(LogReader):
        def sample_weight(self, thread_id, sample_time):
            times.append(sample_time)
            return super().sample_weight(thread_id, sample_time)

    state = LogReaderState(DEFAULT_MAX_SAMPLE_GAP)
    with path.open("rb") as source:
        TimedReader(gunzip(source), state).read_all()
    symbols = dict(state.virtual_ips)
    metadata_file = path.with_suffix(path.suffix + ".meta.json")
    metadata = json.loads(metadata_file.read_text()) if metadata_file.exists() else {}
    profile_os = str(state.meta.get("os", "")).lower()
    wall_timer = profile_os.startswith("win") or state.profile_real_time
    samples = []
    for (trace, estimated_periods, tid, memory_kb), timestamp in zip(
        state.profiles, times
    ):
        stack = []
        for address in trace:
            if address <= 0:
                # Line markers are reversed with the trace; apply them to their frame.
                if stack and -address > 0:
                    stack[-1]["line"] = -address
                continue
            symbol = symbols.get(address)
            frame = {"function": "[unresolved]", "address": hex(address)}
            if symbol:
                parts = symbol.split(":", 3)
                if len(parts) == 4:
                    language, function, line, filename = parts
                    frame["function"] = function
                    if filename and filename != "-":
                        frame["file"] = filename
                    if line.isdigit() and int(line) > 0:
                        frame["line"] = int(line)
                else:
                    frame["function"] = symbol
            stack.append(frame)
        samples.append(
            {
                "timestamp_us": timestamp // 1000 if timestamp is not None else 0,
                "timestamp_available": timestamp is not None,
                "tid": str(tid),
                "stack": stack,
                "weight": 1,
                "estimated_timer_periods": estimated_periods,
                "rss_bytes": memory_kb * 1024 if state.profile_memory else None,
            }
        )
    return {
        "format": "vmprof-decoded-v1",
        "pid": metadata.get("pid", 0),
        # vmprof's Windows writer uses QueryPerformanceCounter even when the
        # PROFILE_REAL_TIME flag is unset; the raw OS metadata is authoritative.
        "clock": "runtime_monotonic" if wall_timer else "process_cpu",
        "sampling_basis": "wall_timer_stack_snapshots"
        if wall_timer
        else "process_cpu_timer",
        "profile_os": profile_os,
        "clock_alignment": "unmapped",
        "interval_us": state.period,
        "metadata": metadata,
        "lost_time_seconds": state.lost_time,
        "samples": samples,
    }


def file_digest(path):
    digest = hashlib.sha256()
    with path.open("rb") as source:
        while chunk := source.read(1024 * 1024):
            digest.update(chunk)
    return digest.hexdigest()


def dependencies():
    specification = importlib.util.find_spec("vmprof")
    roots = specification.submodule_search_locations if specification else None
    if not roots or len(roots) != 1:
        raise RuntimeError("vmprof decoder requires one identifiable source package")
    package = Path(roots[0]).resolve()
    package_files = {
        path.relative_to(package).as_posix(): file_digest(path)
        for path in sorted(package.rglob("*.py"))
        if path.is_file()
    }
    extensions = {}
    extension_paths = {}
    # Inspect the extension before vmprof's initializer can dlopen it.
    extension = importlib.util.find_spec("_vmprof")
    if extension and extension.origin not in (None, "built-in", "frozen"):
        path = Path(extension.origin).resolve()
        extensions["_vmprof"] = file_digest(path)
        extension_paths["_vmprof"] = str(path)
    return {
        "identity": {
            "name": "vmprof",
            "version": importlib.metadata.version("vmprof"),
            "python_files": package_files,
            "loaded_extensions": extensions,
        },
        "package_root": str(package),
        "extension_paths": extension_paths,
    }


def provenance():
    """Observe decoder files across import, independently of capture metadata."""
    before = dependencies()
    import vmprof.reader

    after = dependencies()
    if before != after or Path(vmprof.reader.__file__).resolve() != Path(
        before["package_root"], "reader.py"
    ):
        raise RuntimeError("vmprof decoder dependencies changed during import")
    script = Path(__file__).resolve()
    executable = Path(sys.executable).resolve()
    return {
        "schema_version": 1,
        "identity": {
            "decoder": "hunin-vmprof-v1",
            "script_sha256": file_digest(script),
            "runtime": {
                "implementation": sys.implementation.name,
                "version": list(sys.version_info),
                "executable_sha256": file_digest(executable),
                "byteorder": sys.byteorder,
                "default_text_encoding": locale.getpreferredencoding(False),
                "flags": {
                    name: getattr(sys.flags, name)
                    for name in dir(sys.flags)
                    if not name.startswith("_")
                    and isinstance(getattr(sys.flags, name), (int, bool))
                },
            },
            "package": before["identity"],
        },
        "evidence": {
            "script": str(script),
            "executable": sys.executable,
            "resolved_executable": str(executable),
            "package_root": before["package_root"],
            "extension_paths": before["extension_paths"],
            "scope": "Observed decoder source, vmprof Python source/extension files across import and decode, and interpreter executable/options. Source loads use a fresh cache namespace. Not a complete OS/standard-library/shared-library snapshot or protection against adversarial file replacement.",
        },
    }


def decode_with_provenance(path):
    if any(
        name in ("vmprof", "_vmprof") or name.startswith("vmprof.")
        for name in sys.modules
    ):
        raise RuntimeError(
            "vmprof decoder requires a fresh process without preloaded vmprof modules"
        )
    # A timestamp-valid stale .pyc must not masquerade as the hashed source.
    prefix = sys.pycache_prefix
    writing = sys.dont_write_bytecode
    with tempfile.TemporaryDirectory(prefix="hunin-vmprof-cache-") as cache:
        try:
            sys.pycache_prefix = cache
            sys.dont_write_bytecode = True
            before = provenance()
            result = decode(path)
            if provenance() != before:
                raise RuntimeError(
                    "vmprof decoder dependencies changed during normalization"
                )
            result["decoder_provenance"] = before
            return result
        finally:
            sys.pycache_prefix = prefix
            sys.dont_write_bytecode = writing


if __name__ == "__main__":
    print(json.dumps(decode_with_provenance(Path(sys.argv[1]))))
