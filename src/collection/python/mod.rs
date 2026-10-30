pub(super) mod cpu_progress;
pub(super) mod graal;
pub(super) mod vmprof;

use anyhow::{Result, bail};

pub(super) fn arguments(args: &[String]) -> Result<(Vec<String>, Vec<String>)> {
    let mut index = 0;
    while let Some(argument) = args.get(index) {
        if argument == "-c" || argument == "-m" || !argument.starts_with('-') {
            break;
        }
        if ["-u", "-O", "-OO", "-B", "-s", "-q", "-v"].contains(&argument.as_str())
            || argument == "--jvm"
            || argument.starts_with("--vm.")
            || argument.starts_with("--python.")
        {
            index += 1;
        } else if argument == "-W" || argument == "-X" {
            if args.get(index + 1).is_none() {
                bail!("{argument} requires a value");
            }
            index += 2;
        } else {
            bail!(
                "Python collector does not support interpreter option {argument}; isolated/no-site/stdin modes require a separate collection route"
            );
        }
    }
    if index >= args.len() {
        bail!("Python collection requires a script, -m module or -c code");
    }
    Ok((args[..index].to_vec(), args[index..].to_vec()))
}
