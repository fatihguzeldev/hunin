"""Published ETW report ownership, bounded polling and cleanup evidence rules."""

import json
from pathlib import PureWindowsPath
import time

# Session::stop accepts these ControlTrace results only after querying that the
# retained session has gone away: SUCCESS, MORE_DATA, ACTIVE_CONNECTIONS and
# WMI_INSTANCE_NOT_FOUND. A request/reason alone is not cleanup evidence.
STOP_RESULTS = frozenset((0, 234, 2402, 4201))


def wait_for_report_state(path, state, deadline):
    """Poll only the published report; readability never substitutes for cleanup."""
    last_error = None
    last_state = None
    while time.monotonic() < deadline:
        try:
            observed = json.loads(path.read_text())
            last_state = observed["state"]
            if last_state == state:
                return observed
        except (FileNotFoundError, PermissionError, json.JSONDecodeError) as error:
            last_error = error
        time.sleep(min(0.02, max(0.0, deadline - time.monotonic())))
    raise RuntimeError(
        f"Published report {path} did not reach {state!r} before its deadline; "
        f"last observed state={last_state!r}; last read error={last_error!r}"
    ) from last_error


def verify_direct_recording(report, wrapper_pid, profile, active_handle):
    assert report.get("schema") == "hunin-xperf-session-v1", report
    assert report.get("state") == "recording", (
        "Direct wrapper was not recording before termination"
    )
    owner = report.get("owner", {})
    assert owner.get("wrapper_pid") == wrapper_pid, report
    birth = owner.get("wrapper_creation_100ns")
    assert type(birth) is int and birth > 0, report
    assert owner.get("profile_output_path") == str(profile), report
    helper_pid = report.get("helper_pid")
    assert type(helper_pid) is int and helper_pid > 0, report
    sessions = report.get("sessions")
    assert isinstance(sessions, list) and sessions, report
    kernels = [
        session for session in sessions if session.get("name") == "NT Kernel Logger"
    ]
    assert len(kernels) == 1, report
    kernel = kernels[0]
    assert kernel.get("log_file") == str(
        PureWindowsPath(profile).with_suffix(".kernel.etl")
    ), report
    assert active_handle != "absent", (
        "Direct wrapper never owned an active kernel session"
    )
    assert int(kernel["control_handle"], 16) == int(active_handle, 16) > 0, report


def verify_direct_cleanup(
    before, after, wrapper_pid, profile, active_handle, remaining_handle
):
    """Check ownership and actual cleanup independently of the shutdown route."""
    verify_direct_recording(before, wrapper_pid, profile, active_handle)
    assert after.get("schema") == before["schema"], after
    assert after.get("state") == "stopped", (
        "Direct wrapper death left unconfirmed ETW cleanup"
    )
    assert after.get("owner") == before["owner"], (
        "Cleanup report belongs to a different wrapper lifetime"
    )
    assert after.get("helper_pid") == before["helper_pid"], (
        "Cleanup report belongs to a different helper"
    )
    assert after.get("sessions") == before["sessions"], (
        "Cleanup report changed the owned sessions"
    )
    events = after.get("events", [])
    for session in before["sessions"]:
        stopped = [
            event.get("details", {})
            for event in events
            if event.get("state") == "session_stopped"
            and event.get("details", {}).get("session") == session
        ]
        assert stopped, ("Missing owned-session stop evidence", session)
        for result in stopped:
            status = result.get("stop_result")
            assert (
                result.get("state") == "stopped"
                and type(status) is int
                and status in STOP_RESULTS
            ), result
    assert remaining_handle == "absent", (
        "Direct wrapper death left an owned ETW session active"
    )
