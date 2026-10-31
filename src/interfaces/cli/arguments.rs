//! Command grammar and literal environment argument decoding.
use clap::{Parser, Subcommand};
use std::{net::SocketAddr, path::PathBuf};

#[derive(Parser)]
#[command(
    version,
    about = "profiling and observability for coding agents. (and developers too)"
)]
pub(super) struct Cli {
    #[arg(long, env = "HUNIN_DATA_DIR", global = true)]
    pub(super) data_dir: Option<PathBuf>,
    /// Select a project by stable ID or unambiguous name.
    ///
    /// Defaults to the launch workspace (run --cwd uses that workspace).
    /// Generic call parameters and MCP workloads do not switch projects.
    #[arg(
        long,
        env = "HUNIN_PROJECT",
        global = true,
        conflicts_with = "data_dir"
    )]
    pub(super) project: Option<String>,
    /// Root containing registry.sqlite and project stores.
    #[arg(
        long,
        env = "HUNIN_REGISTRY_DIR",
        global = true,
        conflicts_with = "data_dir"
    )]
    pub(super) registry_dir: Option<PathBuf>,
    #[command(subcommand)]
    pub(super) command: Command,
}
#[derive(Subcommand)]
pub(super) enum Command {
    /// Describe compiled contracts without opening a recording store.
    BuildInfo,
    /// Check runtime availability and prerequisites without creating a project.
    Capabilities,
    /// Discover operation names; pass a name for its complete public schema.
    Catalog {
        operation: Option<String>,
    },
    /// Manage project discovery without opening every recording database.
    Projects {
        #[command(subcommand)]
        command: ProjectCommand,
    },
    Run {
        /// Print complete operation JSON, including full source/provenance lists.
        /// The default is a compact recording summary with inspection commands.
        #[arg(long)]
        full: bool,
        #[arg(long)]
        name: Option<String>,
        #[arg(long, value_parser = runtime_names(), ignore_case = true)]
        runtime: Option<String>,
        /// Explicit deeper collection; ordinary CPU recording remains the default.
        #[arg(long, value_parser = ["automatic-cpu", "code-origin"])]
        capture_mode: Option<String>,
        #[arg(long, requires = "capture_mode")]
        code_compiler_export: bool,
        #[arg(long, requires = "capture_mode")]
        code_function_filter: Option<String>,
        /// Limit printed Wasm disassembly to this index in each module.
        #[arg(long, requires = "code_compiler_export")]
        code_wasm_function_index: Option<u32>,
        #[arg(long, requires = "capture_mode")]
        code_sampling_interval_us: Option<u32>,
        #[arg(long, requires = "capture_mode")]
        code_max_artifact_bytes: Option<u64>,
        #[arg(long)]
        cwd: Option<PathBuf>,
        #[arg(long)]
        timeout: Option<f64>,
        #[arg(long, default_value_t = 0)]
        expected_exit_code: i32,
        #[arg(long, value_parser = parse_environment)]
        env: Vec<(String, String)>,
        #[arg(long = "input-file")]
        input_files: Vec<PathBuf>,
        #[arg(long, conflicts_with = "expected_stdout_sha256")]
        expected_stdout: Option<String>,
        #[arg(long)]
        expected_stdout_sha256: Option<String>,
        #[arg(required = true, last = true)]
        command: Vec<String>,
    },
    /// Rerun a recording with automatic function counts (sampling remains the recording default).
    Count(super::count::CountOptions),
    List,
    Inspect {
        run_id: String,
    },
    Compare {
        before: String,
        after: String,
    },
    Import {
        path: PathBuf,
        #[arg(long)]
        format: String,
        #[arg(long)]
        run_id: Option<String>,
    },
    /// Invoke a public operation. Use `hunin catalog` to list names or
    /// `hunin catalog <operation>` for its parameters and contract.
    Call {
        /// Emit one shared operation error JSON object to stderr on failure.
        #[arg(long)]
        json_errors: bool,
        operation: String,
        #[arg(default_value = "{}")]
        params: String,
    },
    Mcp,
    Ui {
        #[arg(long, default_value = "127.0.0.1:7878")]
        bind: SocketAddr,
        #[arg(long, env = "HUNIN_UI_DIR")]
        ui_dir: Option<PathBuf>,
    },
}

fn runtime_names() -> clap::builder::PossibleValuesParser {
    clap::builder::PossibleValuesParser::new(
        crate::collection::registered_runtimes()
            .iter()
            .flat_map(|runtime| std::iter::once(runtime.id).chain(runtime.aliases.iter().copied()))
            .chain(std::iter::once("none"))
            .collect::<Vec<_>>(),
    )
}

#[derive(Subcommand)]
pub(super) enum ProjectCommand {
    List,
    /// Create a project with an independent store.
    Create {
        name: String,
        #[arg(long)]
        workspace: Option<PathBuf>,
    },
    /// Register an existing store; its recording data is kept in place.
    /// Stop processes using that store before adoption or moving it; restart after registration.
    Register {
        name: String,
        #[arg(long)]
        store: PathBuf,
        #[arg(long)]
        workspace: Option<PathBuf>,
    },
}

fn parse_environment(value: &str) -> std::result::Result<(String, String), String> {
    let (key, value) = value.split_once('=').ok_or("expected KEY=VALUE")?;
    if key.is_empty() || key.contains('\0') || value.contains('\0') {
        return Err("environment names must be nonempty and contain no NUL bytes".into());
    }
    Ok((key.to_owned(), value.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_capture_flags_leave_literal_target_arguments_untouched() {
        let parsed = Cli::try_parse_from([
            "hunin",
            "run",
            "--capture-mode",
            "code-origin",
            "--code-compiler-export",
            "--code-function-filter",
            "kernel*",
            "--code-wasm-function-index",
            "16",
            "--",
            "node",
            "target.mjs",
            "--capture-mode",
            "target-value",
        ])
        .unwrap();
        let Command::Run {
            capture_mode,
            code_compiler_export,
            code_function_filter,
            code_wasm_function_index,
            command,
            ..
        } = parsed.command
        else {
            panic!("expected run")
        };
        assert_eq!(capture_mode.as_deref(), Some("code-origin"));
        assert!(code_compiler_export);
        assert_eq!(code_function_filter.as_deref(), Some("kernel*"));
        assert_eq!(code_wasm_function_index, Some(16));
        assert_eq!(
            command,
            ["node", "target.mjs", "--capture-mode", "target-value"]
        );
        assert!(
            Cli::try_parse_from([
                "hunin",
                "run",
                "--code-compiler-export",
                "--",
                "node",
                "target.mjs"
            ])
            .is_err()
        );
    }
}
