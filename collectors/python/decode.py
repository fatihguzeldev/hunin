"""Decode vmprof 0.6 recordings without inventing wall-clock sample times."""

import json
from pathlib import Path
import sys


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
    for (trace, estimated_periods, tid, memory_kb), timestamp in zip(state.profiles, times):
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
        samples.append({
            "timestamp_us": timestamp // 1000 if timestamp is not None else 0,
            "timestamp_available": timestamp is not None,
            "tid": str(tid),
            "stack": stack,
            "weight": 1,
            "estimated_timer_periods": estimated_periods,
            "rss_bytes": memory_kb * 1024 if state.profile_memory else None,
        })
    return {
        "format": "vmprof-decoded-v1", "pid": metadata.get("pid", 0),
        # vmprof's Windows writer uses QueryPerformanceCounter even when the
        # PROFILE_REAL_TIME flag is unset; the raw OS metadata is authoritative.
        "clock": "runtime_monotonic" if wall_timer else "process_cpu",
        "sampling_basis": "wall_timer_stack_snapshots" if wall_timer else "process_cpu_timer",
        "profile_os": profile_os,
        "clock_alignment": "unmapped", "interval_us": state.period,
        "metadata": metadata, "lost_time_seconds": state.lost_time,
        "samples": samples,
    }


if __name__ == "__main__":
    print(json.dumps(decode(Path(sys.argv[1]))))
