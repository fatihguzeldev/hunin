//! CLI entry, machine-error projection and operation dispatch.
mod arguments;
mod cancellation;
mod count;
mod dispatch;

use anyhow::Result;
use arguments::{Cli, Command};
use clap::Parser;
use dispatch::run_cli;

pub async fn run() -> Result<()> {
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
                let failure = crate::operations::describe_failure(&operation, &error);
                eprintln!("{}", serde_json::to_string(&failure)?);
                std::process::exit(1);
            }
            Err(error)
        }
    }
}
