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
    Capabilities,
    /// Manage project discovery without opening every recording database.
    Projects {
        #[command(subcommand)]
        command: ProjectCommand,
    },
    Run {
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        runtime: Option<String>,
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
