# Recordings and storage

Hunin keeps recording metadata in SQLite and original inputs in a managed content-addressed artifact directory. The default data home is the operating system's local application-data directory: `~/Library/Application Support/hunin` on macOS, `$XDG_DATA_HOME/hunin` (normally `~/.local/share/hunin`) on Linux, and `%LOCALAPPDATA%\hunin` on Windows. `--data-dir` or `HUNIN_DATA_DIR` selects another home.

`hunin.sqlite` contains run manifests, accepted observations, result selections and dependencies, analyzer registrations and attempt history. `artifacts/` holds immutable SHA256-addressed bytes; run-owned references retain each file's name and format. Equal bytes can be shared without merging the roles of stdout, stderr or other inputs. Source text is retained before launch within the documented size limits. Verified source bytes do not establish a compiled-code/source mapping.

`captures/<run-id>/` is a recording's working capture directory. `analysis-packages/` contains reusable managed analyzer code, while `analysis-executions/<attempt-id>/` contains isolated dependency/build workspaces and retained execution files. Working files are separate from accepted observations. A failed, interrupted or partially valid attempt cannot publish partial output as accepted results.

## Restart and cancellation

Completed recordings and accepted results reopen without rerunning the workload. Reimporting unchanged raw inputs does not duplicate their costs. A new normalization or analyzer result replaces its selected predecessor while preserving history; dependent results become unselected when their actual inputs change.

Recordings and analyzer attempts store their owning process identity and OS process start identity. Another client can persist a cancellation request; the owner stops its own process tree and finalizes status. A service shutdown stops and waits for its owned work. Another live service's recordings remain owned by that service.

Recovery marks a running operation interrupted only when the owner is proven absent or its OS process lifetime changed. It does not signal a stale recorded PID. Unknown ownership remains explicit. Raw evidence is preserved where possible, and interrupted output is never automatically accepted.

## Remove, back up and upgrade

Remove a completed recording with `hunin call runs.remove '{"run_id":"..."}'`. Cancel active recordings or analyzer attempts first. Removal releases observations, run-owned documents, capture directories and analyzer attempt workspaces. Shared artifact bytes remain until their final reference is removed. Reusable analyzer registrations and packages remain available for other recordings.

For a filesystem backup, stop the UI service and all MCP/CLI clients using that data home, let owned operations finish cancellation, and copy the complete home. Include the database, any SQLite `-wal`/`-shm` files present, artifacts and analyzer packages. Copying only the database cannot restore referenced raw inputs.

Schema migrations run transactionally when opening the data home. An older Hunin refuses a newer database schema instead of downgrading its version. Preserve a backup before an upgrade; uninstalling or replacing the executable does not require deleting recordings.
