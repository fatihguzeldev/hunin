# Collectors and importers

External tools can submit Hunin observations through the versioned [observation schema](../protocol/record.schema.json). They use the same acceptance boundary as the bundled collectors, SDKs and analyzers. Keep original profiler inputs and the measurement basis; writing directly to the private SQLite tables bypasses validation and ownership.

## Collect alongside a supervised command

`hunin run` supplies `HUNIN_RUN_ID` and `HUNIN_RECORD_DIR` to its command and inherited children. A producer can write UTF-8 `.jsonl` files into that directory, one complete observation per line. Use a unique file per producer/process and a distinct `producer_id`; synchronize concurrent writers or write separate files. Finish and close output before the supervised command exits. Hunin retains these files and validates the complete spool before publishing it atomically. A malformed or rejected spool remains an artifact with a diagnostic; a successful target exit alone does not establish accepted observations.

For example, save this standard-library Python wrapper as `wall_collector.py`. It measures the interval spent launching and waiting for a child command, including supervision overhead. It does not measure the child's CPU time or individual function durations.

```python
import json, os, pathlib, subprocess, sys, time, uuid

run_id = os.environ['HUNIN_RUN_ID']
directory = pathlib.Path(os.environ['HUNIN_RECORD_DIR'])
producer = 'wall-wrapper-' + str(uuid.uuid4())
epoch_us = time.time_ns() // 1_000
start_ns = time.monotonic_ns()
child = subprocess.Popen(sys.argv[1:])
status = child.wait()
record = {
    'schema_version': 1, 'kind': 'span', 'id': producer + ':wait',
    'run_id': run_id, 'producer_id': producer, 'pid': child.pid,
    'tid': 'process', 'timestamp_us': epoch_us,
    'duration_us': (time.monotonic_ns() - start_ns) // 1_000,
    'name': 'external.supervised_command', 'basis': 'observed',
    'attributes': {
        'scope': 'child_launch_and_wait', 'child_returncode': status,
        'clock': 'unix_epoch', 'clock_alignment': 'aligned',
        'duration_clock': 'monotonic', 'thread_id_source': 'process_scope',
    },
}
(directory / (producer + '.jsonl')).write_text(json.dumps(record) + '\n', encoding='utf-8')
sys.exit(status if status >= 0 else 1)
```

```sh
hunin run --runtime none -- python3 wall_collector.py python3 -c 'print("observed")'
hunin call timeline.query '{"run_id":"<returned-run-id>","kind":"span","limit":100}'
```

`runtime none` selects supervision and baseline resources without starting a bundled CPU collector. Spool ingestion still runs. A wrapper that launches another profiler must retain its own raw inputs and report the target's outcome separately from the profiler's exit. Hunin only sees the supervised wrapper's exit in this route. Cancellation may interrupt the wrapper before it closes its observations; do not emit a completed interval when its end is unknown. The [SDK guide](sdk.md) describes the existing language bindings and their flush/exit guarantees.

## Import an existing recording

The shared `recordings.import` operation accepts `path`, `format`, an optional `name`, and an optional existing `run_id`. CLI and MCP call that same operation. Supported raw format names include `samply`, `v8-cpuprofile`, `vmprof`, `python-cpu-progress`, `graal-cpusampler`, `go-pprof` and `go-trace`. Preserve their associated metadata, symbol, source and clock sidecars beside the original input when importing.

```sh
hunin import /path/to/profile.cpuprofile --format v8-cpuprofile
```

Omitting `run_id` creates an imported recording with unknown execution correctness. The raw input is retained before normalization. Matching bytes and sidecars replay idempotently; accepted results and retained inputs reopen without the target. `recordings.reanalyze` takes a retained `artifact_id` and `run_id` to normalize managed inputs again. Inspect its `accepted`, `summary` and `diagnostics` fields. The [source guide](source-resolution.md) explains mapping provenance and offline identity limits.

For a format Hunin does not decode, an external converter can produce canonical `hunin-jsonl`. Every record must already contain the exact existing recording's `run_id`; JSONL import does not rewrite identities or infer which recording owns the data. Choose the completed recording whose target produced the observations, retain the converter version and original input digest in observation attributes, and import the normalized file:

```sh
hunin import /path/to/normalized.jsonl --format hunin-jsonl --run-id <existing-run-id>
```

Use globally distinct observation IDs within the recording. References may target accepted records in that same recording or other records in the submitted batch, including forward references. Schema violations, wrong-run identities, duplicate IDs, missing references, invalid parent/context kinds and ownership cycles reject the batch. Reimporting an identical accepted input is a no-op. An unrelated recording must remain a separate run; sharing timestamps does not establish common ownership.

Use integer microseconds within the schema's safe integer range. Preserve CPU sample weights and units; elapsed intervals use `duration_us`. A `sample` has `basis: "sampled"`; direct observations, explicit instrumentation and derived conclusions retain their respective bases. When a clock cannot be mapped to the recording epoch, set `attributes.clock_alignment` to `"unmapped"` and preserve its origin/mapping limits. Such records remain inspectable without being plotted as epoch timestamps. Keep process lifetime, producer and thread identifier namespaces explicit rather than merging reused PIDs or unrelated thread IDs.

## Add a built-in adapter

The external routes above require no application-name condition in Hunin. A new built-in automatic mode additionally needs an executable collection plan and precise prerequisite diagnostics in `src/collection/`, a decoder under `src/importers/`, and raw/sidecar identity handling through `ImportedProfile` and `recordings::publish`. Extend the capability report, support matrix and relevant real host/runtime controls together. Decoders return common observations and provenance; they do not write private SQL or derive a second UI model. Keep a small real raw fixture and verify malformed inputs, unresolved clocks/mappings, stable replay and removal/reopening behavior.

For domain-specific relationships computed from an existing recording, use a [managed analyzer](analysis.md). Its declared inputs, applicability, cancellation and atomic result selection are separate from collecting observations.

## Page retained metadata

`analyses.list`, `analyses.results`, `artifacts.list` and `scenarios.list` accept `limit` (1–500, default 100) and an optional `cursor`. Each response retains its usual array and includes `next_cursor`; pass that cursor unchanged to request the next page. Stop when it is null or absent. `analyses.results` and `artifacts.list` are scoped to the required `run_id`. Analyzer registrations and attempts are ordered by creation time, while updated scenario recipes use their save time. Artifacts use stable artifact ID order. These pages are live views; refresh from the first page after concurrent insertions or recipe changes.

The UI exposes previous/next/first-page controls for these lists. Polling an active attempt refreshes its selected page. Starting a new attempt opens the newest attempt page so its progress remains visible.
