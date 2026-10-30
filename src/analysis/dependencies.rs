use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Component, Path},
};

fn dependency_file(source: &Path, name: &str) -> Result<()> {
    let path = Path::new(name);
    if path.is_absolute()
        || path
            .components()
            .any(|p| !matches!(p, Component::Normal(_) | Component::CurDir))
    {
        bail!("dependency files must be within the registered package");
    }
    if !source.join(path).is_file() {
        bail!("declared dependency file was not packaged: {name}");
    }
    Ok(())
}
pub fn validate(dependencies: &Value, source: &Path) -> Result<()> {
    for (key, value) in dependencies
        .as_object()
        .context("dependencies must be an object")?
    {
        match key.as_str() {
            "python_requirements" | "node_lockfile" => dependency_file(
                source,
                value
                    .as_str()
                    .context("dependency file path must be a string")?,
            )?,
            "setup_commands" => {
                for command in value
                    .as_array()
                    .context("setup_commands must be an array")?
                {
                    if command.as_array().is_none_or(|args| {
                        args.is_empty()
                            || args
                                .iter()
                                .any(|arg| arg.as_str().is_none_or(str::is_empty))
                    }) {
                        bail!("setup commands must be nonempty argument arrays");
                    }
                }
            }
            _ => bail!("unknown dependency declaration: {key}"),
        }
    }
    if let Some(lock) = dependencies["node_lockfile"].as_str() {
        if Path::new(lock).file_name().and_then(|v| v.to_str()) != Some("package-lock.json") {
            bail!("node_lockfile must name package-lock.json for npm ci");
        }
        let package = source.join(lock).parent().unwrap().join("package.json");
        if !package.is_file() {
            bail!("package.json must be packaged alongside package-lock.json");
        }
        let parsed: Value = serde_json::from_slice(&fs::read(source.join(lock))?)?;
        if parsed["lockfileVersion"].as_u64().is_none_or(|v| v < 2) {
            bail!("npm dependency installation requires package-lock.json version 2 or newer");
        }
    }
    Ok(())
}
pub async fn prepare(
    analysis: &Value,
    workspace: &Path,
    execution: &Path,
    control: super::process::Control,
) -> Result<(Vec<String>, Value)> {
    let mut command: Vec<String> = serde_json::from_value(analysis["command"].clone())?;
    let declarations = &analysis["dependencies"];
    let mut commands: Vec<(Vec<String>, std::path::PathBuf)> = Vec::new();
    if let Some(requirements) = declarations["python_requirements"].as_str() {
        let environment = crate::host::tool_path(&workspace.join(".hunin-python"));
        commands.push((
            vec![
                command[0].clone(),
                "-c".into(),
                // venv captures ensurepip's combined output, but its CLI only
                // prints CalledProcessError's summary. Keep the actual failure
                // in our managed setup stderr rather than losing that evidence.
                "import subprocess,sys,venv\ntry:\n venv.main()\nexcept subprocess.CalledProcessError as error:\n sys.stderr.buffer.write(error.output or b''); raise\n".into(),
                environment.display().to_string(),
            ],
            workspace.to_path_buf(),
        ));
        let python = environment.join(if cfg!(windows) {
            "Scripts/python.exe"
        } else {
            "bin/python"
        });
        commands.push((
            vec![
                python.display().to_string(),
                "-m".into(),
                "pip".into(),
                "install".into(),
                "--require-hashes".into(),
                "--disable-pip-version-check".into(),
                "--no-input".into(),
                "-r".into(),
                crate::host::tool_path(&workspace.join(requirements))
                    .display()
                    .to_string(),
            ],
            workspace.to_path_buf(),
        ));
        command[0] = python.display().to_string();
    }
    if let Some(lock) = declarations["node_lockfile"].as_str() {
        let source = workspace.to_owned();
        let npm = super::blocking(move || {
            super::package::executable(if cfg!(windows) { "npm.cmd" } else { "npm" }, &source)
        })
        .await?;
        commands.push((
            vec![
                npm.display().to_string(),
                "ci".into(),
                "--ignore-scripts".into(),
                "--no-audit".into(),
                "--no-fund".into(),
            ],
            workspace.join(lock).parent().unwrap().to_path_buf(),
        ));
    }
    if let Some(setup) = declarations["setup_commands"].as_array() {
        for step in setup {
            commands.push((
                serde_json::from_value(step.clone())?,
                workspace.to_path_buf(),
            ));
        }
    }
    let mut steps = Vec::new();
    for (index, (arguments, cwd)) in commands.iter().enumerate() {
        let program = arguments[0].clone();
        let source = cwd.clone();
        let check = control.clone();
        let identity = super::blocking(move || {
            check.check()?;
            let executable = super::package::executable(&program, &source)?;
            let identity =
                json!({"path":executable,"sha256":super::files::hash(&executable, || check.check())?});
            check.check()?;
            Ok(identity)
        })
        .await?;
        let status = super::process::execute(
            arguments,
            cwd,
            None,
            &execution.join(format!("setup-{index}.stdout")),
            &execution.join(format!("setup-{index}.stderr")),
            control.clone(),
        )
        .await?;
        steps.push(json!({"command":arguments,"executable":identity,"exit_code":status.code()}));
        let destination = execution.join("dependencies.json");
        let metadata = serde_json::to_vec_pretty(&steps)?;
        let check = control.clone();
        super::blocking(move || {
            check.check()?;
            fs::write(destination, metadata)?;
            check.check()
        })
        .await?;
        if !status.success() {
            bail!(
                "managed dependency step {index} failed; inspect setup-{index}.stderr for the missing prerequisite or lockfile/install error"
            );
        }
    }
    Ok((command, json!({"declarations":declarations,"steps":steps})))
}
