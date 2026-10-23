# Durable custom analyzers

An analyzer is an ordinary local program with a managed package, declared applicability and a versioned input snapshot. It may use a different language from the observed application. Register its code and dependency files through `analyses.register`, then start an execution with `analyses.run`.

Registration copies the package into Hunin's data directory, records every file digest and the selected executable digest, and retains the declared version and dependency instructions. The default package snapshot excludes `.git`, dependency/build directories, Python caches and `.env`. Supply `files` to select a smaller package explicitly. Files must remain inside the package; traversal and external symlinks are rejected. The package limit is 2,000 files and 64 MiB.

A registration request has this shape:

```json
{
  "name": "queue relationships",
  "version": "1",
  "cwd": "/path/to/analyzer-package",
  "command": ["python3", "analyze.py"],
  "files": ["analyze.py", "requirements.lock"],
  "requirements": {
    "runtime": "node",
    "source_fingerprint": "the measured source fingerprint"
  },
  "dependencies": { "python_requirements": "requirements.lock" }
}
```

The command is an argument array. Hunin passes it directly to the process API. A copied executable can be invoked with a relative path such as `./analyze`; interpreter programs are resolved and identified separately. Source and lock files remain in managed storage when the original package directory changes or disappears.

## Applicability

`runtime` matches the observed runtime. Use an explicit `"*"` for a runtime-independent analyzer. A source-sensitive analyzer declares a known `source_fingerprint`; a generic analyzer declares `source_independent: true`. Empty or unknown applicability is rejected. Optional `runtime_version`, `executable_sha256` and `artifact_digests` pin relevant runtime, executable or input-artifact identities.

Declare every condition needed by the analysis. A source fingerprint alone does not establish which binary was built from that source. Build-sensitive offsets need the relevant executable/artifact identities and mapping evidence. Passing declared checks constrains reuse; it cannot prove that the declarations are complete or that the interpretation is correct.

A new recording gets a fresh snapshot and a fresh execution. Old addresses are never copied into the new run automatically. Hunin verifies the managed package and executable before reuse and checks that the selected input snapshot still applies before publishing output. Changed source, runtime or artifact requirements stop execution with an actionable error. Register a revised package when its code, dependencies or valid scope changes.

## Input and output

The analyzer receives a UTF-8 JSON object on stdin containing `run`, `records` and `artifacts`. `HUNIN_ANALYSIS_INPUT` points to the retained copy. Artifact entries include their digest, format and local path. Input metadata also identifies the analyzer package and selected result sets.

Write one complete observation per stdout line using [the canonical observation schema](../protocol/record.schema.json). Send logs to stderr. Every output observation must have `basis: "derived"` and a nonempty `evidence` array referencing actual input record IDs from the same recording. Derived records can reference one another through the ordinary relationship/context fields; acceptance validates the whole output together.

```python
import json, sys, uuid

value = json.load(sys.stdin)
# Application-specific interpretation chooses these from actual input evidence.
origin, destination = select_validated_message_pair(value['records'])
print(json.dumps({
    'schema_version': 1, 'kind': 'relation', 'id': str(uuid.uuid4()),
    'run_id': value['run']['id'], 'producer_id': 'my-analyzer-v1',
    'pid': origin['pid'], 'tid': origin['tid'],
    'timestamp_us': origin['timestamp_us'], 'basis': 'derived',
    'from': origin['id'], 'to': destination['id'],
    'relation': 'dispatches', 'evidence': [origin['id'], destination['id']],
}))
```

`select_validated_message_pair` represents the analyzer's own domain logic. Interval overlap or matching function names alone do not establish a message relationship. Record unknown or ambiguous cases explicitly instead of inventing links.

Only a successful exit with fully valid output can publish an accepted result. Invalid JSON, invalid references, missing evidence, timeout, cancellation and nonzero exit retain their input/stdout/stderr and diagnostics without publishing partial observations. Input snapshots are capped at 128 MiB; captured stdout/stderr share a 16 MiB budget across setup and execution. The default timeout is 60 seconds and can be set up to one day. These limits supervise the process tree and captured output; analyzer code runs with the local user's filesystem and network permissions.

## Dependencies

Dependencies are installed in a fresh managed working copy for each attempt. Registered source and the observed application remain unchanged.

- `python_requirements` names a packaged requirements file. Hunin creates a virtual environment with the selected interpreter and installs using `pip --require-hashes`. All third-party requirements, including transitive dependencies, must be pinned with hashes. An empty lock file is valid for a standard-library-only package. [pip hash-checking mode](https://pip.pypa.io/en/stable/topics/secure-installs/)
- `node_lockfile` names a packaged `package-lock.json` version 2 or newer, with its adjacent `package.json`. Hunin runs `npm ci --ignore-scripts` in that directory. [npm ci](https://docs.npmjs.com/cli/v11/commands/npm-ci/)
- `setup_commands` contains explicit argument arrays for other dependency managers or build tools. They run in the managed working copy under the same cancellation, timeout and output budget. Package the corresponding lock files and required sources, and pin external dependencies. Hunin retains each step's command, executable digest, exit status and output.

Missing interpreters, unavailable package managers, failed locked installs and build prerequisites appear as failed attempts with retained diagnostics. Hunin does not silently fall back to globally installed dependencies.

## Execution history and selection

`analyses.run` returns immediately with `result_set_id`, `status: "running"` and diagnostics. Use that execution ID with `analyses.cancel`. Cancellation requests persist in storage so another connected Hunin service can request cancellation from the owning service. Inspect progress and terminal results with `analyses.results` for the recording.

Terminal states include `accepted`, `replayed`, `failed`, `cancelled` and `timed_out` and `interrupted`. A replay retains its new attempt history and points to the existing accepted result set, preserving idempotency. Result records and accepted execution metadata are published in one transaction. A new accepted result selects that analyzer's current output; previous observations remain in history. Replacing an input invalidates dependent derived results. The `selected` flag identifies which retained result is currently active for queries.

The same accepted observations appear in Timeline, Evidence and common queries. Inspect their input evidence and package identity when evaluating a derived claim.

A service shutdown cancels and waits for its own executions. Confirmed dead-owner attempts become interrupted with partial evidence retained. Unknown process identity is not treated as proof of death, and recovery never signals a reused or unresolved PID.
