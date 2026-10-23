# Installation

Hunin bundles contain the application, its local UI, a ready native profiler, collector adapters, SDK sources, the observation protocol, and documentation. Start with a bundle built for the machine you will profile. The candidate host set is Windows x64, macOS x64/ARM64, and glibc Linux x64/ARM64. A successful bundle build establishes compilation and packaging; it does not establish every collector/runtime combination.

The [support evidence](support.md) distinguishes observed runtime/host cells from CI candidates, including the current GraalPy Intel Mac exclusion. The [collection paths](../collectors/collection-paths.md) describe collection routes, prerequisites, and actual execution evidence. Runtime detection and `capabilities` report missing tools and permissions on the current machine. Windows ARM64 and musl Linux are outside the current candidate set.

## Build from source

Use a current stable Rust toolchain supporting edition 2024, Node.js 22.12 or newer, npm, Git, and Python 3.12 or newer for the build helpers. SQLite is bundled with the Rust executable. Node/npm are build tools for the UI; serving a built UI does not require them. The packaged native profiler is built from pinned samply source and Hunin's checked-in lifetime patches; installed users do not need Rust to run it.

```sh
cd ui
npm ci --ignore-scripts
npm run build
cd ..
cargo build --locked --release
python3 collectors/native/build.py --output collectors/native/bin/samply --cache "$HOME/.cache/hunin/native-build"
```

Package the host-built executable, selecting the matching platform:

```sh
python3 distribution/package.py --binary target/release/hunin --platform macos-arm64
```

The other platform values are `macos-x64`, `linux-x64`, `linux-arm64`, and `windows-x64`. On Windows use `collectors/native/bin/samply.exe` for the profiler build output, a user-owned cache path, and `target/release/hunin.exe` for packaging. The output archive and its `.sha256` file are written to `dist/`. Packaging verifies the profiler's target, binary checksum, upstream pin, and exact patch identity before accepting it. CI builds native bundles for the five candidate hosts and retains them as workflow artifacts. A workflow artifact is an unsigned development build, not a signed release.

Each manifest retains the packaging host's actual OS release, kernel/build version, Linux distribution and libc where available. It also records the source revision and dirty state, GitHub commit/run URL when present, and the explicitly supplied Rust build provenance. For example, add `--rust-toolchain stable --rust-target aarch64-apple-darwin` only when that toolchain and target produced the executable. Omitted or unavailable provenance stays `unknown`; packaging does not infer the compiler from the filename. The tested runner baseline and its linked workflow evidence do not certify an older minimum OS or a successful collection cell. The manifest links the bundled runtime matrix and permission requirements.

## Use an extracted bundle

Check the archive digest against its accompanying checksum, then extract it into a user-owned directory. On Unix, `shasum -a 256 archive.tar.gz` or `sha256sum archive.tar.gz` prints the digest. On PowerShell, use `Get-FileHash archive.zip -Algorithm SHA256`. Checksums detect corruption; authenticity depends on obtaining both files from a trusted build.

Keep the executable beside `ui/`, `collectors/`, `sdk/`, and `protocol/`. You can invoke it directly from this directory. Its adjacent assets travel with the executable; moving the executable alone loses these assets.

An optional installer checks every bundled file against `manifest.json` before copying the bundle into a versioned user directory. It requires Python 3.11 or newer and does not modify your shell configuration or user PATH.

```sh
cd hunin-<version>-<platform>
python3 install.py --bin-dir "$HOME/.local/bin"
```

Unix installs default to `~/.local/share/hunin/<version>-<platform>` and the optional bin directory receives a symlink to the installed executable. Choose `--prefix /your/path` to use another location. Existing version directories and unrelated executables or dangling links are preserved. An existing `hunin` symlink is updated only when it points to an intact, verified prior Hunin installation under that same prefix.

On Windows, run `py install.py`. The default is `%LOCALAPPDATA%\hunin\<version>-windows-x64`; add the printed directory to your user PATH or invoke `hunin.exe` directly. The installer does not request administrator privileges.

## Collector prerequisites

- Native Rust/C/C++ and Go automatic stack collection uses the adjacent patched `samply` executable; retain debug symbols and measured binaries. Its build provenance and upstream licenses are beside the executable under `collectors/native/bin/`. Linux additionally needs a usable perf configuration, Windows ETW collection may need elevation, and macOS restricts protected system targets.
- Node.js and Deno use their V8 CPU profiling routes. Bun uses its own JavaScriptCore profile route. Their control and worker coverage differ; consult the collection path documentation before selecting a mode.
- On Unix, CPython and PyPy require the supported `vmprof` package in the target interpreter. Decoding uses the configured decoder interpreter or Hunin-managed environment; it does not replace the target interpreter. On Windows the CPU route uses OS-thread CPU progress and Python stack sampling without a vmprof dependency; verify the current host evidence and its native attribution gaps in the support guide. Imported Windows vmprof wall snapshots remain distinct from CPU samples.
- GraalPy requires its CPU sampler tooling and a compatible distribution/execution mode. An interpreter name alone is not a capability guarantee.

Always inspect capabilities before recording on a new host. Missing permissions and collectors must remain visible rather than producing an apparently successful empty CPU profile.

The bundle also includes `fetch_samply.py`, an optional helper for the unmodified official samply 0.13.1 release. This upstream binary lacks Hunin's lifetime sidecar extension; use the bundled patched profiler for that collection contract. The helper verifies the release archive against a checked-in SHA256, retains its license files, and installs into the explicit directory you provide:

```sh
python3 fetch_samply.py --platform macos-arm64 --destination /your/user-owned/tools/samply
```

Set `HUNIN_SAMPLY` to the printed executable path, or put its directory on PATH. A matching verified installation is reused; another existing directory is preserved. The helper does not change profiler permissions. Its source and pinned releases are available in [samply's official repository](https://github.com/mstange/samply/releases/tag/samply-v0.13.1).

## UI, agents, and data

Run `hunin capabilities` to inspect prerequisites, `hunin run --cwd /your/project -- node app.js` to record, and `hunin list` to find retained run identifiers. `hunin inspect <run-id>` and `hunin compare <before-id> <after-id>` return structured results. `hunin call <operation> '<JSON parameters>'` exposes the shared operation catalog. Start an agent's stdio MCP connection with `hunin mcp`.

Run `hunin ui` and open `http://127.0.0.1:7878`. See [the local UI guide](ui.md) for recording navigation, evidence, analyzer packages, and comparisons. The local service is a user-owned foreground process; stopping it ends local serving. Data and collector activity have separate lifetimes, described by the CLI and collection operations.

The SDKs are installable from the bundled sources: JavaScript through the local package directory, Python through `pip install /path/to/sdk/python`, Go through its module with a local `replace` when using a checkout, Rust through a path dependency, and C/C++ through the bundled headers and implementation. The [SDK guide](sdk.md) covers APIs, context propagation, installation, and flush/exit semantics.

Upgrading the executable does not require deleting recordings. Keep old bundles until the replacement has opened your data successfully. Do not remove Hunin's data directory as part of upgrading or uninstalling the executable.
