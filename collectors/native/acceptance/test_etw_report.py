import copy
import errno
import json
from pathlib import Path, PureWindowsPath
import unittest
from unittest.mock import patch

from . import etw_report as sessions

HERE = Path(__file__).resolve().parents[1]


class PublishedReportPollingTests(unittest.TestCase):
    def setUp(self):
        self.path = Path("owned/cpu.samply.xperf-session.json")
        self.now = 0.0
        self.monotonic = patch.object(
            sessions.time, "monotonic", side_effect=lambda: self.now
        )
        self.sleep = patch.object(sessions.time, "sleep", side_effect=self.advance)
        self.monotonic.start()
        self.sleep.start()
        self.addCleanup(self.monotonic.stop)
        self.addCleanup(self.sleep.stop)

    def advance(self, seconds):
        self.now += seconds

    def test_transient_permission_denial_requires_the_published_requested_state(self):
        for state, old_state in (("recording", "preflight"), ("stopped", "recording")):
            self.now = 0.0
            denied = PermissionError(errno.EACCES, "Permission denied", str(self.path))
            with (
                self.subTest(state=state),
                patch.object(
                    Path,
                    "read_text",
                    autospec=True,
                    side_effect=[
                        denied,
                        FileNotFoundError(str(self.path)),
                        "{",
                        json.dumps({"state": old_state}),
                        json.dumps({"state": state, "owner": {"wrapper_pid": 7}}),
                    ],
                ) as read,
            ):
                report = sessions.wait_for_report_state(self.path, state, 2.0)
                self.assertEqual(report["state"], state)
                self.assertEqual(read.call_count, 5)
                self.assertTrue(
                    all(call.args == (self.path,) for call in read.call_args_list)
                )

    def test_persistent_permission_denial_fails_at_same_deadline_with_actual_error(
        self,
    ):
        for deadline in (2.0, 8.0):
            self.now = 0.0
            denied = PermissionError(errno.EACCES, "Permission denied", str(self.path))
            with (
                self.subTest(deadline=deadline),
                patch.object(Path, "read_text", side_effect=denied),
                self.assertRaises(RuntimeError) as raised,
            ):
                sessions.wait_for_report_state(self.path, "stopped", deadline)
            self.assertIs(raised.exception.__cause__, denied)
            self.assertIn("PermissionError", str(raised.exception))
            self.assertIn("13", str(raised.exception))
            self.assertEqual(self.now, deadline)

    def test_unrelated_io_error_is_not_retried(self):
        error = OSError(errno.EIO, "unrelated I/O failure")
        with (
            patch.object(Path, "read_text", side_effect=error) as read,
            self.assertRaises(OSError) as raised,
        ):
            sessions.wait_for_report_state(self.path, "stopped", 8.0)
        self.assertIs(raised.exception, error)
        self.assertEqual(read.call_count, 1)
        self.assertEqual(self.now, 0.0)

    def test_readable_nonterminal_report_never_becomes_a_cleanup_acknowledgement(self):
        with (
            patch.object(Path, "read_text", return_value='{"state":"recording"}'),
            self.assertRaisesRegex(RuntimeError, "last observed state='recording'"),
        ):
            sessions.wait_for_report_state(self.path, "stopped", 8.0)
        self.assertEqual(self.now, 8.0)


class WindowsSessionEvidenceTests(unittest.TestCase):
    def setUp(self):
        # Actual EOF-first report from run40075860699/job120818930492. Its
        # cleanup succeeded without a process-watch stop_requested event.
        self.after = json.loads(
            (HERE / "fixtures" / "etw_eof_cleanup.json").read_text()
        )
        self.before = copy.deepcopy(self.after)
        self.before["state"] = "recording"
        self.before["events"] = self.before["events"][:3]
        self.pid = 8468
        self.profile = PureWindowsPath(self.before["owner"]["profile_output_path"])

    def check(self, active="0xffff", remaining="absent"):
        sessions.verify_direct_cleanup(
            self.before, self.after, self.pid, self.profile, active, remaining
        )

    def test_captured_eof_first_cleanup_is_valid_without_a_watch_reason(self):
        self.assertFalse(
            any(event["state"] == "stop_requested" for event in self.after["events"])
        )
        self.check()

    def test_process_watch_cleanup_is_also_valid(self):
        self.after["events"].insert(
            3,
            {
                "state": "stop_requested",
                "details": {"reason": "samply_client_exited"},
                "observed_at_us": 1792828724800000,
            },
        )
        self.check()

    def test_report_read_retry_never_substitutes_another_wrapper_owner(self):
        foreign = copy.deepcopy(self.after)
        foreign["owner"]["wrapper_pid"] = 123
        with (
            patch.object(
                Path,
                "read_text",
                side_effect=[
                    PermissionError(errno.EACCES, "Permission denied"),
                    json.dumps(foreign),
                ],
            ),
            patch.object(sessions.time, "sleep"),
        ):
            report = sessions.wait_for_report_state(
                Path("owned-report.json"), "stopped", sessions.time.monotonic() + 8
            )
        with self.assertRaises(AssertionError):
            sessions.verify_direct_cleanup(
                self.before, report, self.pid, self.profile, "0xffff", "absent"
            )

    def test_requires_live_recording_and_same_actual_kernel_handle(self):
        mutations = [
            lambda report: report.update(state="preflight"),
            lambda report: report.update(sessions=[]),
            lambda report: report["owner"].update(wrapper_pid=123),
            lambda report: report["owner"].pop("wrapper_creation_100ns"),
            lambda report: report["owner"].update(
                profile_output_path="another-run.json"
            ),
            lambda report: report.update(helper_pid=None),
            lambda report: report["sessions"][0].update(log_file="another-run.etl"),
        ]
        original = copy.deepcopy(self.before)
        for index, mutate in enumerate(mutations):
            with self.subTest(mutation=index):
                self.before = copy.deepcopy(original)
                mutate(self.before)
                with self.assertRaises(AssertionError):
                    self.check()
        self.before = original
        for active in ("absent", "0xfffe"):
            with self.subTest(active=active), self.assertRaises(AssertionError):
                self.check(active=active)

    def test_final_report_cannot_substitute_another_owner_or_session(self):
        mutations = [
            lambda report: report["owner"].update(
                wrapper_creation_100ns=134373023242232791
            ),
            lambda report: report["owner"].update(wrapper_pid=123),
            lambda report: report.update(helper_pid=123),
            lambda report: report["sessions"][0].update(control_handle="0xfffe"),
            lambda report: report["sessions"][0].update(log_file="another-run.etl"),
            lambda report: report.update(state="cleanup_failed"),
        ]
        original = copy.deepcopy(self.after)
        for index, mutate in enumerate(mutations):
            with self.subTest(mutation=index):
                self.after = copy.deepcopy(original)
                mutate(self.after)
                with self.assertRaises(AssertionError):
                    self.check()

    def test_every_owned_session_needs_terminal_successful_stop_evidence(self):
        stopped = self.after["events"][4]["details"]
        for status in (None, -1, 5, 99999, "0", False):
            with self.subTest(status=status):
                stopped["stop_result"] = status
                with self.assertRaises(AssertionError):
                    self.check()
        stopped.pop("stop_result")
        with self.assertRaises(AssertionError):
            self.check()
        stopped["stop_result"] = 0
        stopped["state"] = "unknown"
        with self.assertRaises(AssertionError):
            self.check()
        stopped["state"] = "stopped"
        self.after["events"].pop(4)
        with self.assertRaises(AssertionError):
            self.check()

    def test_native_accepted_stop_results_still_require_actual_absence(self):
        for status in (0, 234, 2402, 4201):
            with self.subTest(status=status):
                self.after["events"][4]["details"]["stop_result"] = status
                self.check()
                with self.assertRaises(AssertionError):
                    self.check(remaining="0xffff")

    def test_stop_request_does_not_substitute_for_cleanup(self):
        self.after["events"] = [
            {
                "state": "stop_requested",
                "details": {"reason": "samply_client_exited"},
            }
        ]
        with self.assertRaises(AssertionError):
            self.check()

    def test_success_for_one_session_cannot_hide_another_owned_session(self):
        additional = {
            "name": "Hunin.User.8468",
            "control_handle": "0xfffe",
            "log_file": str(self.profile.with_suffix(".user.etl")),
        }
        self.before["sessions"].append(copy.deepcopy(additional))
        self.after["sessions"].append(copy.deepcopy(additional))
        with self.assertRaises(AssertionError):
            self.check()


if __name__ == "__main__":
    unittest.main()
