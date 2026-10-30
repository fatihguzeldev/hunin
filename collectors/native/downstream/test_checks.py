from pathlib import Path
import subprocess
import unittest
from unittest.mock import patch

from . import checks


class RequiredSourceChecksTest(unittest.TestCase):
    def test_host_selection_keeps_new_guards_in_their_actual_packages(self):
        windows = checks.required_checks("x86_64-pc-windows-msvc")
        selected = {(check.package, check.filter) for check in windows}
        self.assertIn(
            (
                "samply-symbols",
                "gnu_zlib_sections_reject_missing_truncated_mismatched_and_unbounded_data",
            ),
            selected,
        )
        for name in (
            "startup_stop_interrupts_pending_command_before_its_timeout",
            "explicit_stop_interrupts_locked_ipc_without_waiting_for_eof",
        ):
            self.assertIn(("samply", name), selected)
        reports = [
            check
            for check in windows
            if check.filter == "windows::session_report::tests"
        ]
        self.assertEqual(len(reports), 1)
        self.assertEqual(reports[0].expected_passed, 4)
        self.assertEqual(
            checks.required_checks(
                "x86_64-pc-windows-msvc",
                [
                    "windows::sample_correlation",
                    "startup_stop_interrupts_pending_command_before_its_timeout",
                ],
            ),
            windows,
        )
        for target in ("aarch64-apple-darwin", "x86_64-unknown-linux-gnu"):
            selected = checks.required_checks(target)
            self.assertEqual(selected[0].package, "samply-symbols")
            self.assertFalse(any("windows" in check.filter for check in selected))
        for target in ("x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu"):
            clocks = [
                check
                for check in checks.required_checks(target)
                if check.filter == "linux::profiler::hunin_clock_tests"
            ]
            self.assertEqual(len(clocks), 1)
            self.assertEqual(clocks[0].expected_passed, 2)
            wakeups = [
                check
                for check in checks.required_checks(target)
                if check.filter == "linux::perf_event::hunin_wakeup_tests"
            ]
            self.assertEqual(len(wakeups), 1)
            self.assertEqual(wakeups[0].expected_passed, 1)

    def test_zero_passing_or_merely_ignored_tests_cannot_pass_a_required_guard(self):
        for stdout in (
            "test result: ok. 0 passed; 0 failed; 0 ignored;\n",
            "test result: ok. 0 passed; 0 failed; 2 ignored;\n",
        ):
            with self.subTest(stdout=stdout):
                result = subprocess.CompletedProcess([], 0, stdout=stdout)
                with patch.object(checks.subprocess, "run", return_value=result):
                    with self.assertRaisesRegex(ValueError, "passed 0 tests"):
                        checks.run_check(
                            Path("source"),
                            {},
                            "stable",
                            "host",
                            checks.pe_dwarf_check(),
                        )

    def test_pe_integration_enables_ignored_tests_and_requires_both_oracles(self):
        for count in (1, 2):
            with self.subTest(count=count):
                output = f"test result: ok. {count} passed; 0 failed; 0 ignored;\n"
                result = subprocess.CompletedProcess([], 0, stdout=output)
                with patch.object(checks.subprocess, "run", return_value=result) as run:
                    arguments = (
                        Path("source"),
                        {"HUNIN_PE_DWARF_FIXTURE": "real.exe"},
                        "stable",
                        "host",
                        checks.pe_dwarf_check(),
                    )
                    if count == 1:
                        with self.assertRaisesRegex(ValueError, "expected 2"):
                            checks.run_check(*arguments)
                    else:
                        self.assertEqual(checks.run_check(*arguments)["passed"], 2)
                    command = run.call_args.args[0]
                    self.assertEqual(command[command.index("-p") + 1], "wholesym")
                    self.assertIn("hunin_pe_dwarf", command)
                    self.assertEqual(
                        command[-4:], ["--", "--color", "never", "--ignored"]
                    )

    def test_source_failure_is_preserved_instead_of_becoming_a_publication(self):
        result = subprocess.CompletedProcess(
            [], 1, stdout="controlled source failure\n"
        )
        with patch.object(checks.subprocess, "run", return_value=result):
            with self.assertRaises(subprocess.CalledProcessError):
                checks.run_check(
                    Path("source"), {}, "stable", "host", checks.pe_dwarf_check()
                )


if __name__ == "__main__":
    unittest.main()
