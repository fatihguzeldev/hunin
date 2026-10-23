# Collection paths and evidence

Hunin supervises the target and collector process, preserves original profiles,
then normalizes them through its shared ingestion contract. Collection is local;
`samply record --save-only` starts no viewer or symbol server. Raw profiles and
their symbol/clock/process sidecars are inputs to one versioned import identity.
Changing a relevant sidecar creates a new normalized result set rather than
silently rewriting old evidence or counting both versions as current.

## Runtime routes

| Runtime selection | Automatic route | Prerequisites and limits |
| --- | --- | --- |
| `native`, `rust`, `c`, `cpp`, `go` | Hunin-patched samply OS stack sampling; processed Firefox profile with symbol and native lifetime sidecars | Bundled `samply 0.13.1-hunin-native1` with verified provenance; stock samply is rejected. Preserve measured binaries and debug information. Linux uses perf permissions, Windows needs WPT/xperf and elevation, macOS restricts protected system-signed targets. CPU/off-CPU sampling differs by OS. |
| `cpython`, `python`, `pypy` | Unix: vmprof statistical Python/native sampling. Windows: OS-thread CPU progress attributed to bounded Python stack observations. | Unix requires vmprof 0.6.x in the selected interpreter. Windows uses the standard library and native thread CPU counters; delayed observations remain unattributed. Native-only threads, GIL-holding native calls and short-lived threads limit Python stack coverage. No interpreter is substituted. |
| `graalpy` | Truffle CPUSampler with JSON self-hit timestamps | The chosen distribution/mode must include the profiler. The exercised Native Image distribution supports it; JVM-hosted builds are a separate compatibility case. Guest thread labels differ from OS thread IDs. |
| `go-test` | `go test -cpuprofile=...` for one package | Uses Go's built-in profiler without application measurement points. Requires a Go toolchain and an ordinary test workload. CPU and trace modes remain separate. |
| `go-trace` | `go test -trace=...`, decoded with the compatible `go tool trace` | Emits scheduler/runtime/GC events, metrics and completed observed ranges. Raw Go M/G identities are retained; they are not invented OS PIDs or causal request links. |
| `node`, `bun`, `deno` | Runtime CPU profiling with worker-aware adapters | See `collectors/javascript/plan.mjs` and its tests. Runtime control is distinct from the shared CPU-profile importer. |

Python script, `-m module` and `-c code` entry points retain arguments and exit
status. Common `-u`, optimization, bytecode, warning and implementation options are
kept before the wrapper. Isolated/no-site/stdin modes are rejected explicitly
rather than silently changing their import semantics.

The native resolver uses retained symbol sidecars for source files, lines,
inlining, module identity and code identity when available. Unknown native frames
remain visible. An imported profile's source path is historical measured data;
it does not establish that the current editor contents match the measured build.
Source-map resolution and runtime-specific telemetry remain distinct capabilities.

## Installing prerequisites

Packaged installations include Hunin's source-pinned native collector beside the
executable, at `collectors/native/bin/samply` (`samply.exe` on Windows), with its
SHA256 provenance receipt and licenses. End users do not need a Rust compiler.
Stock samply does not provide the required module lifetime extension and is
rejected. Developers can build the pinned source and patches with:

```sh
python collectors/native/build.py --output collectors/native/bin/samply --cache /path/to/native-build-cache
```

Use an `.exe` output on Windows. `HUNIN_SAMPLY` explicitly selects a compatible
patched executable; its version, flags and any accompanying receipt are checked.
Otherwise Hunin prefers the bundled executable, then PATH and the platform's
local `hunin/tools/` directory. No permission change is performed by discovery.

Windows additionally requires the [Windows Performance Toolkit](https://learn.microsoft.com/en-us/windows-hardware/test/wpt/)
from the Windows ADK and an elevated token. Hunin discovers `xperf.exe` on PATH
or in the standard `ProgramFiles(x86)` WPT directory. Native Windows recordings
serialize through a Hunin-owned kernel logger lease and refuse an already active
NT Kernel Logger before starting the target. Finish that recording with its
owning tool, then retry. Hunin never issues an unnamed global xperf stop.

On Unix, install vmprof in the exact Python interpreter or virtual environment
that runs the workload; Windows CPython/PyPy CPU collection needs no vmprof:

```sh
python -m pip install 'vmprof>=0.6,<0.7'
```

The vmprof decoder can be selected separately with `HUNIN_VMPROF_PYTHON`; otherwise
Hunin checks its managed `hunin/tools/vmprof` environment and then `python3`/
`python`. Selecting a decoder does not change the target interpreter. Preserve
the `.vmprof.meta.json` process metadata beside the original recording. Graal
profiles use `.meta.json`; native samply profiles use `.syms.json`; JavaScript
profiles use the matching clock metadata emitted by their adapter.

Packaged installations find collectors beside the Hunin executable. Development
checkouts use the repository path. `HUNIN_ASSETS_DIR` selects an explicit asset
root, and `HUNIN_GO` selects a compatible Go trace decoder where needed.

## Time and cost semantics

- Native Firefox/samply samples use the recording's epoch start plus relative
  sample times. The recorded thread CPU deltas remain separate from sample count.
  On Windows, an ETW user stack belongs to a CPU sample only when its process,
  thread and original sample timestamp match exactly. Without that match, Hunin
  retains the SampleProf instruction pointer and marks the caller chain
  unavailable. A later sample's stack is never assigned to earlier samples. Module
  load/lifetime evidence remains separate from caller-stack availability.
- vmprof timestamps use process-CPU time on the default Unix CPU timer. Windows
  uses QueryPerformanceCounter and wall-timer stack snapshots even when its raw
  real-time flag is unset; the decoder reads the raw OS metadata and preserves
  that runtime-monotonic clock. Windows vmprof RSS capture is unavailable; Hunin
  disables it and keeps external process RSS observations separately. PyPy's
  unsupported line tracing is disabled so its setup cannot alter target stdout.
  Both clock domains remain explicitly unmapped to epoch time; a wall-clock
  start/end bracket cannot turn CPU progress into a
  per-sample wall timeline. Every actual recorded stack weighs one sample;
  vmprof's estimated missed timer periods are retained as separate attributes.
- Python CPU-progress samples weigh measured OS thread CPU microseconds, not
  wall time or call duration. CPU windows above 100 ms or stack/counter capture
  latency above 10 ms remain unattributed raw evidence.
- Graal self-hit timestamps are epoch milliseconds. Inclusive node hit counts
  are not summed as self cost; only self hits create samples.
- V8 profile times require the adapter's verified epoch anchor. Raw imports with
  no anchor remain available for hotspots while their clock is unmapped.
- pprof weights are aggregate samples, CPU time or bytes according to the chosen
  raw sample type. Profile collection time is not an individual sample timestamp.
- Go traces with ClockSnapshot map trace time to epoch. Older traces with no
  snapshot remain unmapped. Observed begin/end ranges can produce durations;
  temporal overlap does not create additional causal edges.

## Process ownership and stopping

The recording owner has a persistent PID, OS start-time and instance identity.
CLI, MCP and UI connections request cancellation through the shared store; only
the owning engine signals its process tree. Shutdown stops that engine's work.
An abandoned recording is recovered only when OS absence or a changed process
start identity proves that its owner is gone. Recovery never signals a stale PID.

Cancellation first permits 1.5 seconds of collector/target cleanup, then forces
the owned tree closed. Unix collection owns a new process group; descendants
that deliberately create another session/group are outside that ownership
boundary. Windows launches suspended, assigns a kill-on-close Job Object, and
resumes the primary thread; Windows behavior still requires its host tests.
Original partial/malformed profiles, metadata and diagnostics are preserved even
when normalization fails. Target exit/correctness and collection outcome remain
separate, and the recorded supervised elapsed interval labels sampler overhead.
Windows native cancellation writes an owned-session stop request and waits up to
8 seconds for its helper acknowledgement before forcing the process Job. The
helper polls both that request and a retained handle to the actual Samply IPC
client, validates its process creation time, and stops ETW on the controller's
acquiring thread. Hunin also waits up to 8 seconds for the bound cleanup report
after the supervised tree exits. Retained
`*.xperf-session.json` evidence distinguishes stopped, refused, failed and
unconfirmed cleanup. An unknown cleanup state is not a claim that ETW stopped.
Native imported process spans retain the collector's observed process interval
and distinguish process lifetimes when the raw recording supplies their starts.

On macOS, failure to resume a task after a native image snapshot aborts sampling.
For a launched target, the collector kills and waits for its retained owned child;
attach mode does not kill the attached process.

Outside Git, source identity uses a bounded source/config-file snapshot, excluding
dependency/build/cache and Hunin data directories. Missing/incomplete snapshots
remain unknown. Source and launched executable identity stay distinct; current
editor files are not silently advertised as the measured build's exact sources.

## Exercised evidence

Local macOS ARM64 checks on 2026-10-23 exercised samply 0.13.1 on C (334 samples),
C++ (316) and Rust (308) controls, resolving their named CPU hotspots; CPython
3.14.6 and PyPy 8.0.1 / Python 3.12.15 with vmprof 0.6.0; GraalPy 3.13.14 /
GraalVM CE Native 25.4.4 with source-resolved CPU self hits; and Go 1.27.3 CPU
pprof and runtime execution traces. Exact recordings vary with scheduling and
are evidence for those paths, not an expected stable sample count.

`src/importers/fixtures` preserves small real native, Graal and Go profile inputs.
Focused importer tests check source/clock/unit preservation, malformed references,
sidecar identity, stable replay, and schema acceptance. Python collector tests
run a real CPU control and an exiting module without application instrumentation.
The five proposed product hosts are Windows x64, macOS ARM64/x64 and glibc Linux
ARM64/x64. Windows ARM64 and musl Linux are outside that proposal. Collection on
each host requires actual execution evidence; building an adapter is insufficient.

`collectors/verify.py --binary /path/to/hunin --runtime native --output evidence`
exercises an actual recording through the CLI, checks a known automatic hotspot
and correctness, and reopens the result. Choose each runtime explicitly; missing
prerequisites fail rather than skipping a support cell. `--output` preserves the
workload, store, raw recordings and success/failure evidence for CI inspection.

Mechanism references: [samply](https://github.com/mstange/samply),
[vmprof](https://vmprof.readthedocs.io/en/latest/vmprof.html),
[Graal profiling](https://www.graalvm.org/latest/tools/profiling/),
[Go diagnostics](https://go.dev/doc/diagnostics),
[pprof wire contract](https://github.com/google/pprof/blob/main/proto/profile.proto).
