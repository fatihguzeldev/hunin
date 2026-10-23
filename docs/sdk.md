# Explicit observations

The optional SDKs add spans, named contexts, counters, events and relationships to the same recording as automatic collection. Install an SDK from the Hunin distribution's `sdk/` directory. Hunin does not copy instrumentation files into the observed project.

Hunin supplies `HUNIN_RUN_ID` and `HUNIN_RECORD_DIR` when launching the workload. SDKs become inactive when those variables are absent. Deno also requires permission to read those variables and write the supplied directory; denied environment access disables the JavaScript SDK. Keep these variables intact when launching participating child processes.

## JavaScript and TypeScript

Install the local `sdk/javascript` package with your package manager, then import `@hunin/sdk`. The package contains ESM JavaScript and TypeScript declarations and requires no build step.

```js
import { withContext, withSpan, counter, captureContext, flush } from '@hunin/sdk';

await withContext('request', () => withSpan('decode', async () => {
  counter('items', 1, 'count');
  const token = captureContext(); // JSON-compatible; carry with a queued job
  await processJob(token);
}));
flush();
```

`withSpan` preserves synchronous values, promises and thrown exceptions. AsyncLocalStorage isolates concurrent request scopes within one JavaScript isolate. Carry `captureContext()` explicitly across a worker or process boundary, and restore it at the receiver with `withContext(token, callback)`. The same ESM package is used with Node, Bun and Deno through their Node-compatible APIs; Deno's environment and write permissions still apply. `startSpan(name)` provides manual `end()` and `run(callback)` methods. `event(name, attributes)` and `relation(fromId, toId, name, evidenceIds)` add explicit observations. A span callback receives its span handle while collection is active; check its optional `id` when supporting disabled operation.

## Python

Install `sdk/python` with the target interpreter, for example `python -m pip install /path/to/hunin/sdk/python`. The package is pure Python and works with CPython, PyPy and GraalPy.

```python
import hunin

with hunin.context('request'):
    async with hunin.span('decode'):
        hunin.counter('items', 1, 'count')
        token = hunin.capture_context()
        await process_job(token)
hunin.flush()
```

`hunin.span` supports ordinary `with` and `async with`. Async tasks inherit contextvars; pass `capture_context()` explicitly into a new thread or process and restore it with `with hunin.attach_context(token):`. `hunin.event` and `hunin.relation` record explicit events and links. Forked processes obtain a new producer identity and do not finish an inherited parent span a second time.

## Go

The module is `github.com/fatihguzeldev/hunin/sdk/go`. Use the distributed `sdk/go` directory as a local module replacement until consuming a published version.

```go
ctx := hunin.NewContext(context.Background(), "request", nil)
ctx, span := hunin.Start(ctx, "decode", nil)
defer span.End()
hunin.Counter(ctx, "items", 1, "count", nil)
```

Pass the returned `context.Context` through goroutines. `CaptureContext(ctx)` returns a serializable propagation token and `AttachContext(ctx, token)` restores it. `Event` and `Relation` take the same context. `Flush()` returns any writer error. No background goroutine or implicit shutdown hook is installed; completed records are written synchronously.

## Rust

Add `hunin-sdk = { path = "/path/to/hunin/sdk/rust" }` to the target's dependencies.

```rust
let context = hunin_sdk::Context::new("request");
let span = context.span("decode");
let token = span.context();
hunin_sdk::counter(&token, "items", 1.0, "count");
span.end();
hunin_sdk::flush()?;
```

A span ends on `Drop` or explicit `end()`. Contexts are explicit values that can move between threads and asynchronous tasks. Serialize a `Context` with serde for cross-process propagation. `Span::attribute`, `event` and `relation` add metadata and observations. `flush()` returns `Result<(), String>`.

## C and C++

The bundle includes the C implementation and both public headers, `hunin.h` and `hunin.hpp`. C++ wrappers share the C implementation. A CMake consumer can build the bundled source directly; `hunin_sdk` supplies the include paths and platform thread dependency:

```cmake
cmake_minimum_required(VERSION 3.16)
project(my_workload LANGUAGES C CXX)
add_subdirectory("/absolute/path/to/hunin/sdk/c" "${CMAKE_BINARY_DIR}/hunin-sdk")
add_executable(my_workload main.cpp)
target_compile_features(my_workload PRIVATE cxx_std_17)
target_link_libraries(my_workload PRIVATE hunin_sdk)
```

For a C application, use `main.c` and omit `target_compile_features`; the SDK implementation uses C11. Build the application with debug information when it will also be sampled automatically:

```sh
cmake -S . -B build -DCMAKE_BUILD_TYPE=RelWithDebInfo
cmake --build build --config RelWithDebInfo
```

Run the resulting application through `hunin run --runtime native --cwd /your/project -- /path/to/my_workload` to receive the SDK recording environment. Native CPU sampling additionally needs the profiler and OS permissions described in [installation](installation.md). The SDK remains optional for automatic CPU sampling.

```c
hunin_context_t context = hunin_context_new("request");
hunin_span_t span;
hunin_span_begin(&span, "decode", &context);
hunin_context_t token = hunin_span_context(&span);
hunin_counter(&token, "items", 1, "count");
hunin_span_end(&span);
hunin_flush();
```

```cpp
auto context = hunin::context("request");
{
  hunin::Span span("decode", &context);
  auto token = span.context();
  hunin::counter(token, "items", 1);
}
hunin::flush();
```

Copy a native context into a job to propagate it across threads. Its public string fields can also be carried across a process boundary. Their capacity is `HUNIN_ID_CAPACITY`, including the terminator. A live C span owns its name; finish it once before reusing its storage. C++ spans are noncopyable and finish through RAII. `hunin_event` / `hunin_relation` and their C++ wrappers add observations. `hunin_flush()` returns zero on success.

## Recording semantics

A counter observation is a delta with its stated unit. Contexts group explicit work; they do not allocate process CPU or RSS to a request. A propagated parent identifies the producing span even when a queued child executes after the parent's interval ends. Propagate tokens within the same Hunin run; SDKs ignore tokens for a different run.

A propagation token carries record IDs, not copies of the originating observations. Its referenced context and parent span must also be available in accepted observations for that same recording. Finish the parent span and flush participating producers at shutdown boundaries; a token does not make an unfinished parent span into a completed observation. Explicit context membership and parent links describe declared work relationships, rather than an inferred allocation of sampled CPU or RSS.

Each participating process lifetime has a distinct producer identity; JavaScript worker isolates also have separate producers within a shared PID. Do not merge producer identities merely because numeric PID or thread values match. Thread IDs preserve their source namespace: Node worker ID, Python `threading.get_ident`, Rust `ThreadId`, C `pthread_self` or Windows thread ID. Go leaves the thread ID empty because goroutines can migrate. These values do not establish a mapping to an OS profiler thread without additional evidence.

`contexts.query` returns contexts and relationships from a shared paginated sequence. Follow `next_cursor` until it is null when reconstructing the complete graph; an empty contexts array on a page does not mean that no relationships remain.

Timestamps use epoch microseconds; span durations use monotonic clocks. Records retain clock and thread-source metadata and are validated against `protocol/record.schema.json` during ingestion. Completed records from all participating processes are accepted together, so child records may arrive before their parents. Missing references reject that result set with diagnostics; raw files remain available.

Call `flush()` at explicit shutdown boundaries and inspect `lastError`, `last_error`, `LastError`, `last_error()` or `hunin_last_error()` when diagnosing missing observations. Writer failures do not replace the application's return values or exceptions. Flush does not fabricate an unfinished span. Abrupt termination can lose buffered JavaScript records and active spans; ordinary exit flushes JavaScript and Python, while Go, Rust and native SDKs write completed observations synchronously.
