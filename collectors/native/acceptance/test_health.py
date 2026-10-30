import copy
import unittest

from .health import raw_health, verify_capture_health


WARNING = "Native lifetime evidence is incomplete (lost events)"


class NativeCaptureHealthTest(unittest.TestCase):
    def evidence(self, complete=True):
        sidecar = {
            "schema_version": 1,
            "collector": {"os": "linux"},
            "truncated": False,
            "events": [],
        }
        phase = {"attributes": {"evidence_complete": complete}}
        relation = {
            "relation": "sampled_in_code_instance",
            "attributes": {"evidence_complete": complete},
        }
        run = {"diagnostics": [] if complete else [WARNING]}
        return sidecar, [phase], [relation], run

    def lost(self):
        evidence = self.evidence(False)
        evidence[0]["events"].append(
            {"kind": "loss", "basis": "perf_lost_events", "count": 181}
        )
        return evidence

    def test_healthy_and_reported_loss_are_distinct_verified_contracts(self):
        healthy = verify_capture_health(*self.evidence())
        self.assertTrue(healthy["contract_verified"])
        self.assertTrue(healthy["capture_complete"])
        self.assertEqual(healthy["counted_lost_events"], 0)
        lost = verify_capture_health(*self.lost())
        self.assertTrue(lost["contract_verified"])
        self.assertFalse(lost["capture_complete"])
        self.assertEqual(lost["counted_lost_events"], 181)
        self.assertEqual(lost["loss_signals"], 1)

    def test_loss_cannot_be_hidden_by_complete_phases_or_relations(self):
        for group in (1, 2):
            evidence = self.lost()
            evidence[group][0]["attributes"]["evidence_complete"] = True
            with self.subTest(group=group), self.assertRaises(AssertionError):
                verify_capture_health(*evidence)

    def test_healthy_evidence_cannot_be_arbitrarily_marked_incomplete(self):
        for group in (1, 2):
            evidence = self.evidence()
            evidence[group][0]["attributes"]["evidence_complete"] = False
            with self.subTest(group=group), self.assertRaises(AssertionError):
                verify_capture_health(*evidence)

    def test_unknown_loss_and_malformed_counts_are_rejected(self):
        for count in (True, 0, -1, 1.5, "181", None, 2**64):
            evidence = self.lost()
            evidence[0]["events"][0]["count"] = count
            with self.subTest(count=count), self.assertRaises(AssertionError):
                verify_capture_health(*evidence)
        evidence = self.lost()
        evidence[0]["events"][0]["basis"] = "unexpected_future_loss"
        with self.assertRaises(AssertionError):
            verify_capture_health(*evidence)

    def test_loss_must_remain_visible_and_cannot_pass_without_observations(self):
        evidence = self.lost()
        evidence[3]["diagnostics"] = []
        with self.assertRaises(AssertionError):
            verify_capture_health(*evidence)
        for group in (1, 2):
            evidence = self.lost()
            evidence[group].clear()
            with self.subTest(group=group), self.assertRaises(AssertionError):
                verify_capture_health(*evidence)

    def test_truncation_has_health_semantics_but_live_fixture_still_rejects_it(self):
        evidence = self.evidence(False)
        evidence[0]["truncated"] = True
        self.assertFalse(raw_health(evidence[0])["capture_complete"])
        with self.assertRaises(AssertionError):
            verify_capture_health(*evidence)
        for value in (None, 0, "false"):
            evidence[0]["truncated"] = value
            with self.subTest(value=value), self.assertRaises(AssertionError):
                raw_health(evidence[0])

    def test_known_snapshot_loss_has_unknown_event_count_instead_of_fake_zero(self):
        evidence = self.evidence(False)
        evidence[0]["events"] = [
            {
                "kind": "loss",
                "basis": "proc_maps_snapshot_unavailable",
                "process": {"key": "p1"},
                "time_ms": 123.0,
            }
        ]
        health = verify_capture_health(*evidence)
        self.assertEqual(health["uncounted_loss_signals"], 1)
        self.assertIsNone(health["reasons"][0]["lost_events"])
        evidence[0]["events"][0]["time_ms"] = float("nan")
        with self.assertRaises(AssertionError):
            verify_capture_health(*evidence)

    def test_dyld_reason_and_windows_count_follow_actual_producer_shapes(self):
        evidence = self.evidence(False)
        evidence[0]["collector"]["os"] = "macos"
        evidence[0]["events"] = [
            {
                "kind": "loss",
                "basis": "dyld_snapshot_incomplete",
                "process": {"key": "p1"},
                "time_ms": 123.0,
                "observation_completed_ms": 124.0,
                "previous_observation_ms": 122.0,
                "budget_exhausted": True,
                "reason": "busy_budget_exhausted",
            }
        ]
        self.assertFalse(verify_capture_health(*evidence)["capture_complete"])
        malformed = copy.deepcopy(evidence)
        malformed[0]["events"][0]["reason"] = "unknown"
        with self.assertRaises(AssertionError):
            verify_capture_health(*malformed)
        evidence[0]["collector"]["os"] = "windows"
        evidence[0]["events"] = [
            {
                "kind": "lifecycle",
                "event": "events_lost",
                "time_ms": 12.0,
                "data": {"count": 3},
            }
        ]
        self.assertEqual(verify_capture_health(*evidence)["counted_lost_events"], 3)
        evidence[0]["events"][0]["data"] = {}
        with self.assertRaises(AssertionError):
            verify_capture_health(*evidence)

    def test_missing_raw_contract_or_boolean_flags_are_not_healthy_defaults(self):
        for field in ("schema_version", "truncated", "events", "collector"):
            evidence = self.evidence()
            del evidence[0][field]
            with self.subTest(field=field), self.assertRaises(AssertionError):
                verify_capture_health(*evidence)
        evidence = self.evidence()
        evidence[1][0]["attributes"]["evidence_complete"] = 1
        with self.assertRaises(AssertionError):
            verify_capture_health(*evidence)
