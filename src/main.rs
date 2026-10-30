mod cli;
use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use hunin::{interfaces, model::runs::RunStatus, operations::Operations, storage::Store};
use serde_json::{Value, json};
use std::{net::SocketAddr, path::PathBuf};

#[derive(Parser)]
#[command(
    version,
    about = "profiling and observability for coding agents. (and developers too)"
)]
struct Cli {
    #[arg(long, env = "HUNIN_DATA_DIR", global = true)]
    data_dir: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    /// Describe compiled contracts without opening a recording store.
    BuildInfo,
    Capabilities,
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
    Count(cli::count::CountOptions),
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

fn parse_environment(value: &str) -> std::result::Result<(String, String), String> {
    let (key, value) = value.split_once('=').ok_or("expected KEY=VALUE")?;
    if key.is_empty() || key.contains('\0') || value.contains('\0') {
        return Err("environment names must be nonempty and contain no NUL bytes".into());
    }
    Ok((key.to_owned(), value.to_owned()))
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let machine_operation = match &cli.command {
        Command::Call {
            operation,
            json_errors: true,
            ..
        } => Some(operation.clone()),
        _ => None,
    };
    match run_cli(cli, machine_operation.is_some()).await {
        Ok(()) => Ok(()),
        Err(error) => {
            if let Some(operation) = machine_operation {
                let failure = hunin::operations::describe_failure(&operation, &error);
                eprintln!("{}", serde_json::to_string(&failure)?);
                std::process::exit(1);
            }
            Err(error)
        }
    }
}

async fn run_cli(cli: Cli, machine_errors: bool) -> Result<()> {
    if matches!(cli.command, Command::BuildInfo) {
        println!(
            "{}",
            serde_json::to_string_pretty(&hunin::product::build_info())?
        );
        return Ok(());
    }
    let root = cli.data_dir.unwrap_or_else(|| {
        dirs::data_local_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("hunin")
    });
    let store = Store::open(&root)?;
    let operations = if machine_errors {
        Operations::try_new(store)?
    } else {
        Operations::new(store)
    };
    let value = match cli.command {
        Command::BuildInfo => unreachable!("build-info returns before opening a store"),
        Command::Capabilities => operations.call("capabilities", json!({})).await?,
        Command::Run {
            name,
            runtime,
            cwd,
            timeout,
            expected_exit_code,
            env,
            input_files,
            expected_stdout,
            expected_stdout_sha256,
            command,
        } => {
            let mut params = json!({"command":command,"expected_exit_code":expected_exit_code,"env":env.into_iter().collect::<std::collections::BTreeMap<_,_>>(),"input_files":input_files});
            for (key, value) in [
                ("name", json!(name)),
                ("runtime", json!(runtime)),
                ("cwd", json!(cwd)),
                ("timeout_seconds", json!(timeout)),
                ("expected_stdout", json!(expected_stdout)),
                ("expected_stdout_sha256", json!(expected_stdout_sha256)),
            ] {
                if !value.is_null() {
                    params[key] = value;
                }
            }
            let start = operations.call("runs.start", params).await?;
            let id = start["run"]["id"].as_str().unwrap();
            let run = tokio::select! {run=operations.engine.wait(id)=>run?,_=tokio::signal::ctrl_c()=>{operations.engine.cancel(id).await?;operations.engine.wait(id).await?}};
            let result = operations.call("runs.get", json!({"run_id":id})).await?;
            println!("{}", serde_json::to_string_pretty(&result)?);
            if run.status != RunStatus::Completed {
                std::process::exit(1);
            }
            return Ok(());
        }
        Command::Count(options) => {
            let planning = options.plan;
            let result = cli::count::measure(&operations, options).await?;
            println!("{}", serde_json::to_string_pretty(&result)?);
            if !planning && result["status"] != "complete" {
                std::process::exit(1);
            }
            return Ok(());
        }
        Command::List => operations.call("runs.list", json!({})).await?,
        Command::Inspect { run_id } => {
            operations
                .call("runs.get", json!({"run_id":run_id}))
                .await?
        }
        Command::Compare { before, after } => {
            operations
                .call("comparisons.create", json!({"before":before,"after":after}))
                .await?
        }
        Command::Import {
            path,
            format,
            run_id,
        } => {
            let mut params = json!({"path":path,"format":format});
            if let Some(id) = run_id {
                params["run_id"] = json!(id);
            }
            operations.call("recordings.import", params).await?
        }
        Command::Call {
            operation,
            params,
            json_errors,
        } => {
            let parameters = serde_json::from_str::<Value>(&params)
                .map_err(|error| {
                    if json_errors {
                        hunin::operations::invalid_parameters_error(error)
                    } else {
                        anyhow::Error::new(error)
                    }
                })
                .context("parameters must be a JSON object")?;
            let mut result = operations.call(&operation, parameters).await?;
            if matches!(
                operation.as_str(),
                "runs.start" | "scenarios.run" | "calls.measure"
            ) && result["run"]["status"] == "running"
            {
                let id = result["run"]["id"].as_str().unwrap().to_owned();
                let run = tokio::select! {
                    run=operations.engine.wait(&id)=>run?,
                    _=tokio::signal::ctrl_c()=>{operations.engine.cancel(&id).await?;operations.engine.wait(&id).await?}
                };
                result["run"] = json!(run);
            }
            if operation == "analyses.run" && result["status"] == "running" {
                let id = result["result_set_id"]
                    .as_str()
                    .context("analyzer execution ID missing")?
                    .to_owned();
                result = tokio::select! {
                    result=operations.analyses.wait(&id)=>result?,
                    _=tokio::signal::ctrl_c()=>{operations.analyses.cancel(&id).await?;operations.analyses.wait(&id).await?}
                };
            }
            result
        }
        Command::Mcp => return interfaces::mcp::serve(operations).await,
        Command::Ui { bind, ui_dir } => {
            let adjacent = std::env::current_exe()?.parent().unwrap().join("ui");
            let ui = ui_dir.unwrap_or_else(|| {
                if adjacent.join("index.html").is_file() {
                    adjacent
                } else {
                    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui/dist")
                }
            });
            return interfaces::http::serve(operations, bind, ui).await;
        }
    };
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}
