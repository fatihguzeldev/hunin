//! Bounded collector prerequisite discovery, independent of interface composition.
mod permissions;
mod probe;
mod runtime;

use serde_json::{Value, json};
use std::{env, sync::Arc};

pub(crate) fn discover() -> Value {
    let permission = Arc::new(permissions::inspect());
    let mut workers = Vec::new();
    for (runtime, names) in [
        ("node", vec!["node"]),
        ("bun", vec!["bun"]),
        ("deno", vec!["deno"]),
        ("cpython", vec!["python3", "python"]),
        ("pypy", vec!["pypy3", "pypy"]),
        ("graalpy", vec!["graalpy", "graalpython"]),
        ("go", vec!["go"]),
        ("native", vec!["samply"]),
    ] {
        let permission = permission.clone();
        workers.push((
            runtime,
            std::thread::spawn(move || runtime::inspect(runtime, &names, &permission)),
        ));
    }
    let runtimes = workers
        .into_iter()
        .map(|(runtime,worker)|worker.join().unwrap_or_else(|_|json!({"runtime":runtime,"available":false,"probe_status":"internal_probe_failure","diagnostics":["Capability worker failed; availability is unknown"]})))
        .collect::<Vec<_>>();
    json!({
        "host":{"os":env::consts::OS,"arch":env::consts::ARCH},
        "native_sampling_permission":*permission,
        "runtimes":runtimes,
        "schema_version":1,
        "probe_limits":{"timeout_seconds_per_command":probe::TIMEOUT_SECONDS,"output_bytes_per_stream":probe::OUTPUT_BYTES},
        "verification":"Discovery never labels executable presence as a verified CPU recording.",
    })
}
