# Local UI

The human UI uses the same accepted recordings and application operations as the CLI and MCP server. It renders results prepared by the core; it does not maintain a second attribution or comparison engine. No Perfetto components are used.

Build the UI with `cd ui && npm ci --ignore-scripts && npm run build`. The local service serves the built assets from `ui/dist` during checkout development, or the adjacent `ui/` directory in an installed bundle. An explicit UI directory or `HUNIN_UI_DIR` can select another built directory. Keep the service bound to loopback; recording and analyzer actions execute programs with the local user's permissions.

Start the built UI with `hunin ui`. It binds to `http://127.0.0.1:7878`; `--bind 127.0.0.1:<port>` chooses another loopback port. Stop the foreground service with Ctrl+C. `--ui-dir /path/to/built/ui` selects explicit assets.

For frontend development, first start `hunin ui`, then run `npm run dev` inside `ui/`. Vite binds only `127.0.0.1:5173` and proxies `/api` to port 7878 with the backend's local Host/Origin headers. Before starting either process, check that the intended port is free or reuse a matching existing process. Stop a task-owned development server with Ctrl+C.

## Record and inspect

Use **Record** to enter the working directory and an executable argument array, for example `["node", "app.js"]`. Argument arrays retain exact argument boundaries and do not add an implicit shell. A shell can be invoked explicitly when a workload needs it. Select the runtime or use detection, and set a finite timeout. The run rail retains failed and canceled recordings as well as completed ones. A running recording refreshes its status and offers **Stop recording**.

Use **Scenarios** to save the command, directory, runtime, timeout, environment overrides, declared input files, and correctness expectations as a repeatable workload. The same optional controls appear in **Record**. Input files are fingerprinted; stdout can be checked as exact text (including newlines) or by SHA-256. Recording a saved scenario reuses that recipe; the backend retains each run's actual checks for comparison. An exit code check alone does not validate program output.

**Capabilities** describes collection routes available on the current host and their missing prerequisites. An installed runtime is distinct from a usable collector.

The recording views cover these tasks:

- **Overview** presents elapsed wall time, sample weights and their units, explicit spans, relationships, correctness checks, diagnostics, and hot functions. Selecting a resolved source location opens the source inspector.
- **Timeline** zooms, pans, filters by observation type or process, and pages through accepted observations. Selecting a mark opens its complete record, references, and available source location. The clock selector exposes unmapped observations as evidence without placing them on an epoch timeline. Thread identifier namespaces remain separate unless a shared mapping is established.
- **Stack profile** switches between an expandable call tree and a flamegraph. Width describes backend sample weight, and its unit can be selected explicitly. Click a frame to focus its subtree, then use **Reset stack** to return. Search covers loaded frames. Bounded queries expose node/depth limits, omitted-child drilldowns, sibling pages, and navigation back to a parent scope or all roots.
- **Resources** pages recorded series with their units, scope, clock, producer, and thread namespace and exposes the underlying values. Unmapped timestamps remain inspectable without an epoch plot. A process total remains a process total; overlap does not assign it to a request.
- **Evidence** exposes contexts, graph relationships and their declared basis, raw observation details, and immutable artifact identities. Concurrent work remains separate until a supported edge is recorded.
- **Analyzers** registers managed packages, declares their applicability and dependencies, runs them against a fresh recording, and retains result set identities and diagnostics. Active attempts refresh their status and can be canceled. Choose an exact source build or explicitly declare a source-independent analyzer. Accepted observations appear in the shared timeline and evidence views. Passing applicability checks does not establish semantic correctness.
- **Compare** selects a baseline and candidate, shows backend comparability reasons and correctness results, then displays the measured deltas. Per-metric quality diagnostics and unavailable deltas stay visible when resource sampling is insufficient. An incorrect result or incompatible workload cannot become an unqualified successful optimization.

Source locations are tied to the measured recording. The inspector distinguishes retained source snapshots from current files, verified source bytes from an unverified relationship to the loaded build, and missing source from missing debug information. Backend provenance diagnostics remain visible even when snapshot bytes are verified. The current editor source is not silently presented as the measured build. Missing samples, unavailable observations, failed collection, and derived relationships remain distinct states.

## Navigation and access

Run and view selection live in the URL, so browser Back/Forward restores navigation. Interactive controls are native buttons, inputs, and selects, with visible focus states. The layout uses a horizontal recording rail on narrow screens and scrollable data tables instead of squeezing evidence into unreadable columns. Loading, empty, and operation failure states retain useful explanations and retry controls.

The [analyzer guide](analysis.md) describes package layout, dependency locks, applicability, execution limits, retries, and accepted result selection in more detail.

The service has no remote account flow. Its HTTP boundary must enforce local origins and loopback access. Do not expose it through a public tunnel or bind it to a public interface to share a recording.
