use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use hunin::{interfaces, operations::Operations, storage::Store};
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
        #[arg(required = true, last = true)]
        command: Vec<String>,
    },
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

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let root = cli.data_dir.unwrap_or_else(|| {
        dirs::data_local_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("hunin")
    });
    let operations = Operations::new(Store::open(&root)?);
    let value = match cli.command {
        Command::Capabilities => operations.call("capabilities", json!({})).await?,
        Command::Run {
            name,
            runtime,
            cwd,
            timeout,
            expected_exit_code,
            command,
        } => {
            let mut params = json!({"command":command,"expected_exit_code":expected_exit_code});
            for (key, value) in [
                ("name", json!(name)),
                ("runtime", json!(runtime)),
                ("cwd", json!(cwd)),
                ("timeout_seconds", json!(timeout)),
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
            if run.status != "completed" {
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
        Command::Call { operation, params } => {
            let mut result = operations
                .call(
                    &operation,
                    serde_json::from_str::<Value>(&params)
                        .context("parameters must be a JSON object")?,
                )
                .await?;
            if matches!(operation.as_str(), "runs.start" | "scenarios.run")
                && result["run"]["status"] == "running"
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
