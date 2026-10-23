# Runtime and host evidence

Hunin's native application targets Windows x64, macOS ARM64/x64, and glibc Linux ARM64/x64. Windows ARM64 and musl Linux are outside this set. Building a bundle proves compilation and packaging. A supported collection cell additionally needs a real target recording, useful language-visible observations, retained raw input, correctness checks, SDK ingestion, and reopening/query verification on that host.

## Current collection routes

| Runtime or language | Automatic collection | Candidate hosts |
| --- | --- | --- |
| Rust, C, C++, compiled Go | samply 0.13.1 native stacks and symbols | All five hosts, subject to OS permissions and debug information |
| Go tests / Go execution trace | Go CPU profile / runtime trace | All five hosts |
| Node.js | V8 CPU profile and worker collection | All five hosts |
| Bun | JavaScriptCore CPU profile | All five hosts |
| Deno | V8 inspector CPU profile | All five hosts |
| CPython | vmprof 0.6.0 in the target interpreter | All five hosts; interpreter/package compatibility must pass |
| PyPy | vmprof 0.6.0 and PyPy's sampling support | All five hosts |
| GraalPy Community | Graal CPU sampler with guest Python frames | Windows x64, macOS ARM64, glibc Linux ARM64/x64 |

These are collection routes and CI targets, not a blanket statement that every cell has passed. Consult [collection paths](../collectors/collection-paths.md) for individual runtime coverage and limitations, [SDK documentation](sdk.md) for context/flush contracts, and the matching CI evidence artifact for actual results. `capabilities` distinguishes an installed executable from a usable collection path.

### GraalPy on Intel Macs

Current official GraalPy 25.4.4 native distributions omit macOS x64. GraalVM removed macOS x64 after 25.0.1; GraalPy no longer supports JVM standalone archives from 25.0.3. The current JVM documentation offers an explicitly unsupported emulation mode without native libraries. That mode does not satisfy Hunin's complete runtime floor. Hunin therefore keeps the macOS x64 application target while making no current GraalPy collection support claim on that host. It does not silently select an obsolete runtime or relabel the unsupported Java fallback as full support. [GraalPy release](https://github.com/oracle/graalpython/releases/tag/graal-25.4.4), [GraalVM platform removal](https://www.graalvm.org/release-notes/JDK_25/), [unsupported-platform limitations](https://graalpy.org/jvm-developers/docs/)

## Reproducible prerequisite versions

The checked-in [runtime asset manifest](../distribution/runtime-assets.json) pins official archives and SHA-256 digests for Bun 1.4.4, Deno 2.9.8, PyPy 8.0.1 / Python 3.12.15, and GraalPy Community 25.4.4 / Python 3.13.14. Bun, Deno, and PyPy each have five archives; GraalPy has four plus the explicit Intel Mac unavailability reason. PyPy checksums come from [its official checksum list](https://pypy.org/checksums.html). The other digests come from the respective official release assets: [Bun](https://github.com/oven-sh/bun/releases/tag/bun-v1.4.4), [Deno](https://github.com/denoland/deno/releases/tag/v2.9.8), [GraalPy](https://github.com/oracle/graalpython/releases/tag/graal-25.4.4).

Use Python 3.12 or newer to install a pinned runtime into an explicit user-owned directory:

```sh
python3 distribution/fetch_runtime.py bun --platform macos-arm64 --destination /your/tools/bun
```

The helper verifies the complete archive before extraction, retains bundled licenses and internal library links, and prints the runtime executable path. It changes neither PATH nor OS permissions. A pristine verified installation is reused; another existing directory is preserved. `--archive` verifies and reuses a local copy of the same pinned archive. CI's `--github-env` publishes the explicit `HUNIN_TEST_<RUNTIME>` path. Runtime source archives are prerequisites; they are not redistributed inside Hunin's bundle.

Pinned versions establish repeatable controls. They do not establish a broad minimum-version promise. Similarly, Ubuntu 24.04 or a current macOS CI runner cannot prove an older glibc/macOS/Windows floor. Minimum OS versions remain candidates until verified on those actual systems.

## What has been observed

Local macOS ARM64 executions have exercised native samply, Node, Bun, Deno, CPython, PyPy, GraalPy, Go CPU profiles and Go traces, with language-visible samples or observed runtime ranges and retained raw inputs. The SDK contract control has exercised Node, Bun, Deno, CPython, PyPy, GraalPy, Go, Rust, C, and C++ on that same host. This is local execution evidence, not evidence for the other four hosts.

An actual macOS ARM64 debug bundle was extracted and installed into a versioned user directory. The installed executable found its adjacent collector assets and UI, recorded a Node workload and a symbolized native workload, and reopened their summaries. A debug install does not certify signed release delivery or a clean install on another host.

The [host workflow](../.github/workflows/ci.yml) builds and packages on `windows-2025`, `macos-14` (ARM64), `macos-15-intel`, `ubuntu-24.04`, and `ubuntu-24.04-arm`. These labels are listed by [GitHub's hosted-runner documentation](https://docs.github.com/en/actions/reference/runners/github-hosted-runners). The same five jobs exercise each available runtime and its SDK bridge; the Intel Mac GraalPy exclusion is written explicitly to that job's summary. Collector failures leave their workload, raw captures, store and `failure.json` in the evidence artifact; successful controls include `evidence.json` and `run.json`.

Each job also attempts an extracted-bundle installation into a fresh versioned directory. That control records a real Node workload through the bundled SDK and adjacent collectors, checks exact stdout, resolves its known hotspot, reopens the summary, reads the retained source after its original path is removed, and serves adjacent UI assets from a temporary loopback server. The server is stopped by the control. Compilation, SDK tests, runtime collection, and installation remain separate evidence categories. Do not mark a candidate cell supported from an uploaded executable alone.

Native collection may require Linux perf access or Windows ETW elevation; macOS restricts protected targets. The workflow changes perf settings only inside its ephemeral Linux runners. Bun/Deno worker coverage and Python sampling clocks retain their documented limitations. Source bytes, loaded executable identity, and verified binary-to-source mapping remain distinct.
