# Runtime and host evidence

Hunin's native application targets Windows x64, macOS ARM64/x64, and glibc Linux ARM64/x64. Windows ARM64 and musl Linux are outside this set. Building a bundle proves compilation and packaging. A supported collection cell additionally needs a real target recording, useful language-visible observations, retained raw input, correctness checks, SDK ingestion, and reopening/query verification on that host.

## Current collection routes

| Runtime or language | Automatic collection | Host targets |
| --- | --- | --- |
| Rust, C, C++, compiled Go | Pinned samply with Hunin's native lifetime extension, stacks and symbols | All five hosts, subject to OS permissions and debug information |
| Go tests / Go execution trace | Go CPU profile / runtime trace | All five hosts |
| Node.js | V8 CPU profile and worker collection | All five hosts |
| Bun | JavaScriptCore CPU profile | All five hosts |
| Deno | V8 inspector CPU profile | All five hosts |
| CPython | vmprof 0.6.0 on Unix; OS-thread CPU-progress gated Python stack sampling on Windows | All five hosts; the Windows CPU route and SDK bridge have actual recording evidence |
| PyPy | vmprof 0.6.0 / PyPy sampling on Unix; OS-thread CPU-progress gated Python stacks on Windows | All five hosts; the Windows CPU route and SDK bridge have actual recording evidence |
| GraalPy Community | Graal CPU sampler with guest Python frames | Windows x64, macOS ARM64, glibc Linux ARM64/x64 |

The recorded controls and tested versions are listed below. Consult [collection paths](../collectors/collection-paths.md) for individual runtime coverage and limitations, [SDK documentation](sdk.md) for context/flush contracts, and the matching CI evidence artifact for actual results. `capabilities` distinguishes an installed executable from a usable collection path.

### GraalPy on Intel Macs

Current official GraalPy 25.4.4 native distributions omit macOS x64. GraalVM removed macOS x64 after 25.0.1; GraalPy no longer supports JVM standalone archives from 25.0.3. The current JVM documentation offers an explicitly unsupported emulation mode without native libraries. That mode does not satisfy Hunin's complete runtime floor. Hunin therefore keeps the macOS x64 application target while making no current GraalPy collection support claim on that host. It does not silently select an obsolete runtime or relabel the unsupported Java fallback as full support. [GraalPy release](https://github.com/oracle/graalpython/releases/tag/graal-25.4.4), [GraalVM platform removal](https://www.graalvm.org/release-notes/JDK_25/), [unsupported-platform limitations](https://graalpy.org/jvm-developers/docs/)

## Reproducible prerequisite versions

The checked-in [runtime asset manifest](../distribution/runtime-assets.json) pins official archives and SHA-256 digests for Bun 1.4.4, Deno 2.9.8, PyPy 8.0.1 / Python 3.12.15, and GraalPy Community 25.4.4 / Python 3.13.14. Bun, Deno, and PyPy each have five archives; GraalPy has four plus the explicit Intel Mac unavailability reason. PyPy checksums come from [its official checksum list](https://pypy.org/checksums.html). The other digests come from the respective official release assets: [Bun](https://github.com/oven-sh/bun/releases/tag/bun-v1.4.4), [Deno](https://github.com/denoland/deno/releases/tag/v2.9.8), [GraalPy](https://github.com/oracle/graalpython/releases/tag/graal-25.4.4).

Use Python 3.12 or newer to install a pinned runtime into an explicit user-owned directory:

```sh
python3 distribution/fetch_runtime.py bun --platform macos-arm64 --destination /your/tools/bun
```

The helper verifies the complete archive before extraction, retains bundled licenses and internal library links, and prints the runtime executable path. It changes neither PATH nor OS permissions. A pristine verified installation is reused; another existing directory is preserved. `--archive` verifies and reuses a local copy of the same pinned archive. CI's `--github-env` publishes the explicit `HUNIN_TEST_<RUNTIME>` path. Runtime archives are prerequisites; they are not redistributed inside Hunin's bundle.

Pinned versions establish repeatable controls. They do not establish a broad minimum-version promise. Similarly, Ubuntu 24.04 or a current macOS CI runner cannot prove an older glibc/macOS/Windows floor. An older minimum OS or libc version has not been established.

The native profiler's [source manifest](../collectors/native/manifest.json) pins upstream revision `da75c28f367454c621e690eeb4e44ec2ebb29a78` and the source archive checksum. The build helper applies the included patches, checks the native target and lifetime CLI extension, and retains the compiler, combined patch identity and binary checksum beside the ready executable. Host bundles require a matching binary and provenance. The per-host recordings below exercise that lifetime extension.

## What has been observed

At revision `8eeac27`, all five hosts completed the collection and extracted-bundle controls listed below. These exercised native and ordinary Go sampling with known source positions, the selected JS/Python routes and SDK bridges, target correctness, module load/reload and offline replay, installed CLI/MCP/UI assets, durable analyzers and comparisons, and installed native C SDK ingestion. This table reports those collection and installation controls; the linked jobs also contain separate unit and sampling-mechanism tests.

| Application target | Runner | Measured host | Collection and installation evidence |
| --- | --- | --- | --- |
| Windows x64 | `windows-2025` | Windows Server 2025, build 10.0.26100 | [Observed controls](https://github.com/fatihguzeldev/hunin/actions/runs/40018789537/job/120637640348) |
| glibc Linux ARM64 | `ubuntu-24.04-arm` | Ubuntu 24.04.5 LTS, kernel 6.17.0-1022-azure, glibc 2.39 | [Observed controls](https://github.com/fatihguzeldev/hunin/actions/runs/40018789537/job/120637639766) |
| glibc Linux x64 | `ubuntu-24.04` | Ubuntu 24.04.5 LTS, kernel 6.17.0-1022-azure, glibc 2.39 | [Observed controls](https://github.com/fatihguzeldev/hunin/actions/runs/40018789537/job/120637640544) |
| macOS ARM64 | `macos-14` | macOS 14.8.9, Darwin 23.6.0 | [Observed controls](https://github.com/fatihguzeldev/hunin/actions/runs/40018789537/job/120637640227) |
| macOS x64 | `macos-15-intel` | macOS 15.7.9, Darwin 24.6.0 | [Observed controls](https://github.com/fatihguzeldev/hunin/actions/runs/40018789537/job/120637640245) |

The matching package manifests and retained recordings report these runtime versions. They describe the executed controls, without establishing a minimum-version promise:

| Host group | Node.js | CPython | Go | Hunin Rust compiler |
| --- | --- | --- | --- | --- |
| Windows x64 | 22.23.4 | 3.12.10, OS-thread CPU-progress route | 1.24.13 | 1.99.1 |
| Both Linux architectures | 22.23.3 | 3.12.15 with vmprof 0.6.0 | 1.24.13 | 1.99.1 |
| Both Mac architectures | 22.23.2 | 3.12.10 with vmprof 0.6.0 | 1.24.13 | 1.99.1 |

Bun 1.4.4, Deno 2.9.8 and PyPy 8.0.1 / Python 3.12.15 were observed on all five hosts. PyPy used vmprof 0.6.0 on Unix and the OS-thread CPU-progress route on Windows. GraalPy CE Native 25.4.4.1.1 / Python 3.13.14 was observed on Windows, both Linux architectures and macOS ARM64; the Intel Mac exclusion remains explicit. Rust's compiler version identifies the Hunin application build. The native workload evidence retains its own source, symbols and executable identity.

The five installed profiler receipts in that `8eeac27` evidence set contain seven patches with combined SHA-256 `76b8814192f6fc39d0bc66e50fe2f36ee21dcd0d83fc7b2b5b600b1df688676c`. Its package manifests record source revision and GitHub commit `8eeac27d8ab6a8f0d028c1b3d074c9e899c26e9b`, together with `source_dirty: true` for the packaging workspace; that field does not identify which paths caused the state. For the bundle being used, inspect `collectors/native/bin/samply.provenance.json` (`samply.exe.provenance.json` on Windows) for its actual patch list, combined digest, binary checksum and compiler target, and `manifest.json` for its packaged file digests and build provenance.

The [host workflow](../.github/workflows/ci.yml) builds and packages on `windows-2025`, `macos-14` (ARM64), `macos-15-intel`, `ubuntu-24.04`, and `ubuntu-24.04-arm`. These labels are listed by [GitHub's hosted-runner documentation](https://docs.github.com/en/actions/reference/runners/github-hosted-runners). The same five jobs exercise each available runtime and its SDK bridge; the Intel Mac GraalPy exclusion is written explicitly to that job's summary. Collector failures leave their workload, raw captures, store and `failure.json` in the evidence artifact; successful controls include `evidence.json` and `run.json`.

The extracted-bundle control installs the application into a fresh versioned directory. It records a real Node workload through the bundled SDK and adjacent collectors, checks exact stdout, resolves its known hotspot, reopens the summary, and reads retained source after its original path is removed. It then uses the installed executable's actual stdio MCP interface to discover tools/capabilities, record the same workload, register and execute a managed analyzer after its original script is moved, resolve its accepted evidence, replay it idempotently, and compare CLI/MCP reruns. The MCP transcript is retained and its accepted results reopen through CLI after shutdown. Adjacent UI assets are fetched from a temporary loopback server that the control stops.

A separate installed native control builds a C application against the bundled SDK with CMake and records it with the native profiler. Its retained source calls a known CPU function inside an explicit SDK context/span. Automatic hotspot samples, the SDK observations, their raw input and shared query identities must survive reopening. This controlled source relationship does not establish general per-request CPU ownership or map unrelated thread namespaces.

Compilation, SDK tests, runtime collection, and installation remain separate evidence categories. An uploaded executable alone does not establish collection or installation support.

Native collection requires usable Linux perf access, or the Windows Performance Toolkit (`xperf.exe`) and appropriate ETW privileges; macOS restricts protected targets. The workflow changes perf settings only inside its ephemeral Linux runners. Bun/Deno worker coverage and Python sampling clocks retain their documented limitations. Windows vmprof wall-timer snapshots do not satisfy the Python CPU floor. The Windows CPU-progress route weights actual OS-thread CPU progress and leaves native/GIL-held attribution gaps unassigned; it does not turn a sampled frame into a measured call duration. Source bytes, loaded executable identity, and verified binary-to-source mapping remain distinct.
