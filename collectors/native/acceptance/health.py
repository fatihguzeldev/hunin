"""Independently check declared capture loss against observable normalized quality."""

import math


def positive_count(value):
    assert type(value) is int and 0 < value < 2**64, (
        "Loss count must be an explicit positive u64",
        value,
    )
    return value


def observation_time(value):
    assert type(value) in (int, float) and math.isfinite(value) and value >= 0, (
        "Invalid loss observation time",
        value,
    )
    return value


def snapshot_loss(event):
    process = event.get("process")
    assert isinstance(process, dict) and isinstance(process.get("key"), str), (
        "Snapshot loss must identify its process instance",
        event,
    )
    assert process["key"], event
    return observation_time(event.get("time_ms"))


def raw_health(sidecar):
    assert isinstance(sidecar, dict) and type(sidecar.get("schema_version")) is int
    assert sidecar["schema_version"] == 1, "Unsupported lifetime sidecar"
    assert type(sidecar.get("truncated")) is bool, "Truncation must be explicit"
    collector = sidecar.get("collector")
    assert isinstance(collector, dict), "Lifetime collector identity missing"
    host = collector.get("os")
    assert host in ("linux", "macos", "windows"), "Unknown lifetime producer host"
    events = sidecar.get("events")
    assert isinstance(events, list), "Lifetime event list missing"
    reasons = {}
    for event in events:
        assert isinstance(event, dict) and isinstance(event.get("kind"), str), (
            "Malformed lifetime event",
            event,
        )
        kind = event["kind"]
        if kind == "loss":
            basis = event.get("basis")
            if basis == "perf_lost_events":
                assert host == "linux", event
                count = positive_count(event.get("count"))
            elif basis == "proc_maps_snapshot_unavailable":
                assert host == "linux", event
                snapshot_loss(event)
                count = None
            elif basis == "dyld_snapshot_incomplete":
                assert host == "macos", event
                start = snapshot_loss(event)
                assert observation_time(event.get("observation_completed_ms")) >= start
                previous = event.get("previous_observation_ms")
                assert previous is None or observation_time(previous) <= start
                assert type(event.get("budget_exhausted")) is bool, event
                assert event.get("reason") == (
                    "busy_budget_exhausted"
                    if event["budget_exhausted"]
                    else "snapshot_probe_error"
                ), event
                count = None
            else:
                raise AssertionError(("Unknown native loss basis", event))
        elif event.get("event") == "events_lost":
            assert host == "windows" and kind == "lifecycle", event
            observation_time(event.get("time_ms"))
            data = event.get("data")
            assert isinstance(data, dict), event
            count = positive_count(data.get("count"))
            basis = "events_lost"
        else:
            continue
        reason = reasons.setdefault(
            basis, {"basis": basis, "signals": 0, "lost_events": 0 if count else None}
        )
        reason["signals"] += 1
        if count is not None:
            reason["lost_events"] += count
    return {
        "capture_complete": not sidecar["truncated"] and not reasons,
        "truncated": sidecar["truncated"],
        "loss_signals": sum(reason["signals"] for reason in reasons.values()),
        "counted_lost_events": sum(
            reason["lost_events"] or 0 for reason in reasons.values()
        ),
        "uncounted_loss_signals": sum(
            reason["signals"]
            for reason in reasons.values()
            if reason["lost_events"] is None
        ),
        "reasons": [reasons[basis] for basis in sorted(reasons)],
    }


def verify_capture_health(sidecar, phases, relations, run):
    health = raw_health(sidecar)
    # This small live fixture must never exhaust the bounded sidecar. Generic
    # truncation semantics have a separate deterministic control.
    assert health["truncated"] is False, "Live lifetime fixture sidecar was truncated"
    assert phases and relations, "Missing lifetime/sample-link observations"
    for record in [*phases, *relations]:
        complete = record.get("attributes", {}).get("evidence_complete")
        assert complete is health["capture_complete"], (
            "Normalized completeness contradicts independent raw health",
            record,
            health,
        )
    assert all(
        relation.get("relation") == "sampled_in_code_instance" for relation in relations
    ), "Unexpected relation in native health control"
    diagnostics = run.get("diagnostics")
    assert isinstance(diagnostics, list) and all(
        isinstance(diagnostic, str) for diagnostic in diagnostics
    ), "Run diagnostics missing"
    visible = any(
        diagnostic.startswith("Native lifetime evidence is incomplete (")
        for diagnostic in diagnostics
    )
    assert visible is (not health["capture_complete"]), (
        "Visible run diagnostic contradicts independent raw health",
        diagnostics,
        health,
    )
    return {
        "contract_verified": True,
        **health,
        "checked_module_instances": len(phases),
        "checked_sample_links": len(relations),
        "incomplete_warning_visible": visible,
        "limitation": "Declared loss has no localization guarantee; incomplete intervals do not prove continuous mapping.",
    }
