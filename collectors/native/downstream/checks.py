"""Required regression selections for the pinned downstream source."""

from dataclasses import dataclass
from pathlib import Path
import re
import subprocess


@dataclass(frozen=True)
class SourceCheck:
    package: str
    filter: str
    harness: tuple[str, ...] = ("--bin", "samply")
    ignored: bool = False
    minimum_passed: int = 1
    expected_passed: int | None = None


def required_checks(target, extra_filters=()):
    checks = [
        SourceCheck(
            "samply-symbols",
            "gnu_zlib_sections_reject_missing_truncated_mismatched_and_unbounded_data",
            ("--lib",),
            expected_passed=1,
        )
    ]
    if "windows" in target:
        checks.extend(
            [
                SourceCheck("samply", "windows::sample_correlation"),
                SourceCheck("samply", "sample_stack_tests"),
                SourceCheck(
                    "samply",
                    "startup_stop_interrupts_pending_command_before_its_timeout",
                    expected_passed=1,
                ),
                SourceCheck(
                    "samply",
                    "explicit_stop_interrupts_locked_ipc_without_waiting_for_eof",
                    expected_passed=1,
                ),
                SourceCheck(
                    "samply", "windows::session_report::tests", expected_passed=4
                ),
            ]
        )
    elif "apple" in target:
        checks.extend(
            [
                SourceCheck("samply", "mac::image_snapshot"),
                SourceCheck("samply", "mac::process_launcher"),
            ]
        )
    elif "linux" in target:
        checks.extend(
            [
                SourceCheck(
                    "samply", "linux::profiler::hunin_clock_tests", expected_passed=2
                ),
                SourceCheck(
                    "samply", "linux::perf_event::hunin_wakeup_tests", expected_passed=1
                ),
                SourceCheck(
                    "samply", "linux::observer_health::tests", expected_passed=3
                ),
            ]
        )
    checks.extend(SourceCheck("samply", value) for value in extra_filters)
    selected = {}
    for check in checks:
        key = (check.package, check.filter, check.harness, check.ignored)
        selected.setdefault(key, check)
    return tuple(selected.values())


def pe_dwarf_check():
    return SourceCheck(
        "wholesym",
        "pe_",
        ("--test", "hunin_pe_dwarf"),
        ignored=True,
        expected_passed=2,
    )


def run_check(source, environment, toolchain, target, check):
    command = [
        "cargo",
        f"+{toolchain}",
        "test",
        "--locked",
        "--release",
        "--target",
        target,
        "-p",
        check.package,
        *check.harness,
        check.filter,
    ]
    command.extend(["--", "--color", "never"])
    if check.ignored:
        command.append("--ignored")
    print(f"Required native source check: {check.package}:{check.filter}", flush=True)
    result = subprocess.run(
        command,
        cwd=Path(source),
        env=environment,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        check=False,
        timeout=600,
    )
    print(result.stdout, end="", flush=True)
    result.check_returncode()
    passed = sum(
        int(count)
        for count in re.findall(
            r"^test result: ok\. (\d+) passed;", result.stdout, re.M
        )
    )
    if passed < check.minimum_passed or (
        check.expected_passed is not None and passed != check.expected_passed
    ):
        expected = (
            str(check.expected_passed)
            if check.expected_passed is not None
            else f"at least {check.minimum_passed}"
        )
        raise ValueError(
            f"Required native source check {check.package}:{check.filter} passed {passed} tests; expected {expected}."
        )
    return {
        "package": check.package,
        "filter": check.filter,
        "harness": list(check.harness),
        "ignored_enabled": check.ignored,
        "passed": passed,
    }


def run_checks(source, environment, toolchain, target, checks):
    return [
        run_check(source, environment, toolchain, target, check) for check in checks
    ]
