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
| `native`, `rust`, `c`, `cpp`, `go` | samply OS stack sampling; processed Firefox profile plus symbol sidecar | samply 0.13+; preserve measured binaries and debug information. Linux uses perf permissions, Windows uses ETW/elevation, macOS restricts protected system-signed targets. CPU/off-CPU sampling differs by OS. |
| `cpython`, `python`, `pypy` | vmprof statistical Python/native sampling in the requested interpreter | Install vmprof 0.6+ in that exact interpreter. Python threads are sampled; a subprocess needs its own collection route. No interpreter is silently substituted. |
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

Install [samply's official release](https://github.com/mstange/samply/releases) for
the host, or use `cargo install --locked samply`. Put it on `PATH` or set
`HUNIN_SAMPLY` to its executable. Hunin also finds a previously installed binary
under the platform's local data directory at `hunin/tools/`. No elevated install
or profiler permission change is performed implicitly by capability detection.

For Python, use the interpreter or its virtual environment that runs the workload:

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
- vmprof CPU timestamps are process-CPU time. They remain explicitly unmapped to
  epoch time; a wall-clock start/end bracket cannot turn CPU progress into a
  per-sample wall timeline. Every actual recorded stack weighs one sample;
  vmprof's estimated missed timer periods are retained as separate attributes.
- Graal self-hit timestamps are epoch milliseconds. Inclusive node hit counts
  are not summed as self cost; only self hits create samples.
- V8 profile times require the adapter's verified epoch anchor. Raw imports with
  no anchor remain available for hotspots while their clock is unmapped.
- pprof weights are aggregate samples, CPU time or bytes according to the chosen
  raw sample type. Profile collection time is not an individual sample timestamp.
- Go traces with ClockSnapshot map trace time to epoch. Older traces with no
  snapshot remain unmapped. Observed begin/end ranges can produce durations;
  temporal overlap does not create additional causal edges.

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

Mechanism references: [samply](https://github.com/mstange/samply),
[vmprof](https://vmprof.readthedocs.io/en/latest/vmprof.html),
[Graal profiling](https://www.graalvm.org/latest/tools/profiling/),
[Go diagnostics](https://go.dev/doc/diagnostics),
[pprof wire contract](https://github.com/google/pprof/blob/main/proto/profile.proto).
