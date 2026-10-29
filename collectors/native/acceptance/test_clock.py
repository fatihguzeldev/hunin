import copy
import unittest

from .clock import verify_linux_epoch_windows


class LinuxEpochWindowsTest(unittest.TestCase):
    def evidence(self):
        reference = "443191000100"
        clock = {
            "kind": "clock_reference",
            "clock": "linux_clock_monotonic",
            "basis": "bracketed_clock_read",
            "raw_to_ns_factor": 1,
            "monotonic_before_raw": "443191000000",
            "monotonic_after_raw": "443191000200",
            "clock_reference_raw": reference,
            "epoch_reference_ms": 1793300440000.0,
        }
        events = [clock]
        phases, markers = [], []
        for index in range(3):
            start = 1793300440100000 + index * 400000
            load_id = str(index + 15)
            phases.append(
                {
                    "timestamp_us": start,
                    "duration_us": 230000,
                    "attributes": {"collector_load_id": load_id},
                }
            )
            markers.extend(
                [
                    {"event": "loaded", "phase": index, "epoch_us": start + 1000},
                    {"event": "unloaded", "phase": index, "epoch_us": start + 220000},
                ]
            )
            events.extend(
                [
                    {
                        "kind": "mapping",
                        "load_id": load_id,
                        "clock_reference_raw": reference,
                    },
                    {
                        "kind": "sample",
                        "time_ms": 120.0 + index * 400,
                        "frames": [{"load_id": load_id}],
                    },
                ]
            )
        sidecar = {"epoch_start_ms": clock["epoch_reference_ms"], "events": events}
        run = {"started_at_us": 1793300440000000, "finished_at_us": 1793300442000000}
        return sidecar, run, markers, phases

    def test_valid_phase_and_sample_epochs_use_the_measured_bracket(self):
        proof = verify_linux_epoch_windows(*self.evidence())
        self.assertEqual(proof["allowance_us"], 2)
        self.assertEqual(proof["checked_sequential_phases"], 3)
        self.assertEqual(proof["checked_phase_samples"], 3)

    def test_uptime_added_to_epoch_is_rejected_even_with_matching_relative_order(self):
        evidence = list(self.evidence())
        evidence[0]["events"][2]["time_ms"] += 443191
        with self.assertRaises(AssertionError):
            verify_linux_epoch_windows(*evidence)

    def test_mapped_phase_outside_actual_recording_is_rejected(self):
        evidence = list(self.evidence())
        evidence[3][0]["timestamp_us"] += 443191000
        with self.assertRaises(AssertionError):
            verify_linux_epoch_windows(*evidence)

    def test_clock_allowance_does_not_hide_out_of_window_samples(self):
        evidence = list(self.evidence())
        evidence[0]["events"][2]["time_ms"] = 100.99
        with self.assertRaises(AssertionError):
            verify_linux_epoch_windows(*evidence)

    def test_missing_or_inconsistent_reference_cannot_claim_epoch_alignment(self):
        for change in ("missing", "zero", "different_mapping", "wrong_epoch"):
            with self.subTest(change=change):
                evidence = copy.deepcopy(self.evidence())
                events = evidence[0]["events"]
                if change == "missing":
                    events.pop(0)
                elif change == "zero":
                    events[0]["clock_reference_raw"] = "0"
                elif change == "different_mapping":
                    events[1]["clock_reference_raw"] = "0"
                else:
                    events[0]["epoch_reference_ms"] += 1000
                with self.assertRaises(AssertionError):
                    verify_linux_epoch_windows(*evidence)
