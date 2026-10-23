use serde_json::{Value, json};
use std::{env, process::Command};

pub fn discover() -> Value {
    let mut runtimes = Vec::new();
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
        let found = names.iter().find_map(|name| crate::collection::tool(name));
        let version = found
            .as_ref()
            .and_then(|p| {
                Command::new(p)
                    .arg(if runtime == "go" {
                        "version"
                    } else {
                        "--version"
                    })
                    .output()
                    .ok()
            })
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned());
        let mut diagnostics = Vec::new();
        let mut capabilities = Vec::new();
        if let Some(ref executable) = found {
            if ["cpython", "pypy"].contains(&runtime) {
                let vmprof = Command::new(executable)
                    .args(["-c", "import vmprof"])
                    .output()
                    .is_ok_and(|o| o.status.success());
                if vmprof {
                    capabilities.push("cpu-sampling");
                } else {
                    diagnostics.push("Install vmprof 0.6+ in the selected interpreter for automatic CPU collection".to_owned());
                }
            } else if runtime == "go" {
                if crate::collection::tool("samply").is_some() {
                    capabilities.push("cpu-sampling");
                } else {
                    diagnostics.push("Install samply for automatic Go binary sampling; runtime-enabled go-test/go-trace are separate collection modes".into());
                }
            } else if runtime == "graalpy" {
                let tools = Command::new(executable).arg("--help:tools").output().ok();
                if tools
                    .as_ref()
                    .is_some_and(|o| String::from_utf8_lossy(&o.stdout).contains("cpusampler"))
                {
                    capabilities.push("cpu-sampling");
                } else {
                    diagnostics.push("This GraalPy distribution has not exposed Truffle CPUSampler; select a supported execution mode".into());
                }
            } else {
                capabilities.push("cpu-sampling");
            }
            capabilities.extend(["sdk-records", "process-resources"]);
            diagnostics.push("Installed prerequisites are detected; actual recording may require OS permissions or runtime-specific activation".into());
        } else {
            diagnostics.push(format!(
                "No {} executable found; set HUNIN_{} or install it on PATH",
                names[0],
                names[0].to_uppercase()
            ));
        }
        runtimes.push(json!({"runtime":runtime,"available":found.is_some(),"version":version,"executable":found,"capabilities":capabilities,"diagnostics":diagnostics,"verification":"prerequisites-only"}));
    }
    json!({"host":{"os":env::consts::OS,"arch":env::consts::ARCH},"runtimes":runtimes,"schema_version":1})
}
