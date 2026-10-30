use anyhow::{Context, Result};
use clap::Args;
use hunin::operations::Operations;
use serde_json::{Value, json};

/// Rerun a recording with automatic function counting.
#[derive(Debug, Args)]
pub struct CountOptions {
    pub run_id: String,
    /// Count selected functions. Repeat to select more than one; omit to count all.
    #[arg(long = "function")]
    pub functions: Vec<String>,
    /// Disambiguate each selected function by source path.
    #[arg(long, requires = "functions")]
    pub file: Option<String>,
    /// Disambiguate each selected function by its declaration line.
    #[arg(long, requires = "functions", value_parser = clap::value_parser!(u32).range(1..))]
    pub line: Option<u32>,
    /// Inspect the automatic method and limitations without starting a new run.
    #[arg(long)]
    pub plan: bool,
    /// Permit an explicit counting rebuild.
    #[arg(long)]
    pub allow_rebuild: bool,
    /// Build executable; put its arguments after -- (for example --build cc -- -O2 main.c -o {output}).
    #[arg(long, requires = "allow_rebuild")]
    pub build: Option<String>,
    #[arg(last = true, requires = "build")]
    pub build_args: Vec<String>,
    /// Reuse this key when retrying the same measurement; choose a new key for a fresh run.
    #[arg(long)]
    pub request_id: Option<String>,
    #[arg(long)]
    pub timeout: Option<f64>,
}

impl CountOptions {
    pub fn parameters(&self) -> Value {
        let mut params = json!({"run_id":self.run_id,"allow_rebuild":self.allow_rebuild});
        if !self.functions.is_empty() {
            params["functions"] = self
                .functions
                .iter()
                .map(|name| {
                    let mut function = json!({"name":name});
                    if let Some(file) = &self.file {
                        function["file"] = json!(file);
                    }
                    if let Some(line) = self.line {
                        function["line"] = json!(line);
                    }
                    function
                })
                .collect();
        }
        if let Some(build) = &self.build {
            params["build_command"] = std::iter::once(build)
                .chain(self.build_args.iter())
                .map(String::as_str)
                .collect();
        }
        if let Some(request_id) = &self.request_id {
            params["request_id"] = json!(request_id);
        }
        if let Some(timeout) = self.timeout {
            params["timeout_seconds"] = json!(timeout);
        }
        params
    }
}

/// CLI owns waiting and presentation; the operation owns method selection and jobs.
pub async fn measure(operations: &Operations, options: CountOptions) -> Result<Value> {
    let mut params = options.parameters();
    if options.plan {
        params.as_object_mut().unwrap().remove("timeout_seconds");
        params.as_object_mut().unwrap().remove("request_id");
        return operations.call("calls.plan", params).await;
    }
    let started = operations.call("calls.measure", params).await?;
    let Some(id) = started["run"]["id"].as_str() else {
        return Ok(started);
    };
    let mut report;
    loop {
        report = operations
            .call("calls.query", json!({"run_id":id,"limit":1000}))
            .await?;
        if report["status"] != "running" {
            break;
        }
        tokio::select! {
            _ = tokio::time::sleep(std::time::Duration::from_millis(500)) => {},
            _ = tokio::signal::ctrl_c() => { operations.call("runs.cancel", json!({"run_id":id})).await?; }
        }
    }
    let mut cursor = report["next_cursor"].as_str().map(str::to_owned);
    let mut seen = std::collections::HashSet::new();
    while let Some(next) = cursor {
        anyhow::ensure!(
            seen.insert(next.clone()),
            "function-count pagination did not advance"
        );
        let page = operations
            .call(
                "calls.query",
                json!({"run_id":id,"cursor":next,"limit":1000}),
            )
            .await?;
        let counts = page["counts"]
            .as_array()
            .context("function counts missing")?;
        report["counts"]
            .as_array_mut()
            .context("function counts missing")?
            .extend(counts.iter().cloned());
        cursor = page["next_cursor"].as_str().map(str::to_owned);
    }
    report["next_cursor"] = Value::Null;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::CountOptions;
    use clap::Parser;
    use serde_json::json;

    #[derive(Parser)]
    struct CountCli {
        #[command(flatten)]
        options: CountOptions,
    }

    #[test]
    fn omitted_selection_counts_all_without_enabling_rebuilds() {
        let cli = CountCli::try_parse_from(["count", "parent"]).unwrap();
        assert_eq!(
            cli.options.parameters(),
            json!({"run_id":"parent","allow_rebuild":false})
        );
    }

    #[test]
    fn selected_source_and_explicit_rebuild_preserve_argv_without_shell_expansion() {
        let cli = CountCli::try_parse_from([
            "count",
            "parent",
            "--function",
            "work",
            "--function",
            "parse",
            "--file",
            "src/main.c",
            "--line",
            "12",
            "--allow-rebuild",
            "--build",
            "cc",
            "--",
            "-O2",
            "source with spaces.c",
            "-o",
            "app",
        ])
        .unwrap();
        assert_eq!(
            cli.options.parameters(),
            json!({"run_id":"parent","allow_rebuild":true,"functions":[{"name":"work","file":"src/main.c","line":12},{"name":"parse","file":"src/main.c","line":12}],"build_command":["cc","-O2","source with spaces.c","-o","app"]})
        );
    }

    #[test]
    fn source_filters_and_build_arguments_require_their_explicit_scope() {
        for arguments in [
            vec!["count", "parent", "--file", "main.c"],
            vec!["count", "parent", "--function", "f", "--line", "0"],
            vec!["count", "parent", "--build", "cc"],
            vec!["count", "parent", "--", "-O2"],
        ] {
            assert!(CountCli::try_parse_from(arguments).is_err());
        }
    }
}
