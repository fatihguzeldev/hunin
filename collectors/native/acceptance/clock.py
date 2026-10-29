"""Check live Linux epoch alignment against measured clock and workload evidence."""

import math


def verify_linux_epoch_windows(sidecar, run, markers, phases):
    references = [e for e in sidecar["events"] if e.get("kind") == "clock_reference"]
    assert len(references) == 1, references
    clock = references[0]
    assert clock["clock"] == "linux_clock_monotonic", clock
    assert clock["basis"] == "bracketed_clock_read", clock
    assert clock["raw_to_ns_factor"] == 1, clock
    before = int(clock["monotonic_before_raw"])
    after = int(clock["monotonic_after_raw"])
    reference = int(clock["clock_reference_raw"])
    assert 0 < before <= reference <= after, clock
    assert reference == before + (after - before) // 2, clock
    assert clock["epoch_reference_ms"] == sidecar["epoch_start_ms"], clock
    # The full observed bracket is conservative; one microsecond covers the
    # integer timestamp and floating millisecond projection quantization.
    allowance_us = math.ceil((after - before) / 1000) + 1
    capture_start = run["started_at_us"] - allowance_us
    capture_end = run["finished_at_us"] + allowance_us
    epoch_us = sidecar["epoch_start_ms"] * 1000
    checked_samples = 0
    for index, phase in enumerate(phases):
        loaded = next(
            m for m in markers if m["event"] == "loaded" and m["phase"] == index
        )
        unloaded = next(
            m for m in markers if m["event"] == "unloaded" and m["phase"] == index
        )
        start = phase["timestamp_us"]
        end = start + phase["duration_us"]
        assert capture_start <= start <= loaded["epoch_us"] + allowance_us, phase
        assert unloaded["epoch_us"] - allowance_us <= end <= capture_end, phase
        load_id = phase["attributes"]["collector_load_id"]
        mappings = [
            e
            for e in sidecar["events"]
            if e.get("kind") == "mapping" and e.get("load_id") == load_id
        ]
        assert mappings and all(
            m["clock_reference_raw"] == clock["clock_reference_raw"] for m in mappings
        ), mappings
        samples = [
            e
            for e in sidecar["events"]
            if e.get("kind") == "sample"
            and any(f.get("load_id") == load_id for f in e["frames"])
        ]
        assert samples, phase
        for sample in samples:
            timestamp = epoch_us + sample["time_ms"] * 1000
            assert capture_start <= timestamp <= capture_end, sample
            assert (
                loaded["epoch_us"] - allowance_us
                <= timestamp
                <= unloaded["epoch_us"] + allowance_us
            ), sample
        checked_samples += len(samples)
    return {
        "clock_reference": clock,
        "allowance_us": allowance_us,
        "checked_sequential_phases": len(phases),
        "checked_phase_samples": checked_samples,
        "limitation": "The measured pairing bracket does not bound later wall-clock changes.",
    }
