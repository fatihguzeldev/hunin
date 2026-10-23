use super::StartRequest;
use anyhow::{Result, bail};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{fs::File, io::Read, path::Path};

pub fn recipe(request: &StartRequest) -> Result<Value> {
    if request.expected_stdout.is_some() && request.expected_stdout_sha256.is_some() {
        bail!("choose expected_stdout or expected_stdout_sha256, not both");
    }
    let stdout = match (&request.expected_stdout, &request.expected_stdout_sha256) {
        (Some(text), _) => Some(format!("{:x}", Sha256::digest(text.as_bytes()))),
        (_, Some(hash)) => {
            if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
                bail!("expected_stdout_sha256 must contain exactly 64 hexadecimal digits");
            }
            Some(hash.to_ascii_lowercase())
        }
        _ => None,
    };
    Ok(
        json!({"expected_exit_code":request.expected_exit_code.unwrap_or(0),"expected_stdout_sha256":stdout}),
    )
}

pub fn pending(recipe: &Value) -> Value {
    json!({"passed":null,"expected_exit_code":recipe["expected_exit_code"],"actual_exit_code":null,"validation":if recipe["expected_stdout_sha256"].is_string(){"exit_code_and_stdout"}else{"exit_code_only"},"checks":[]})
}

pub fn evaluate(
    recipe: &Value,
    stdout: &Path,
    exit: Option<i32>,
    canceled: bool,
    timed_out: bool,
) -> Result<Value> {
    let mut file = File::open(stdout)?;
    let mut hash = Sha256::new();
    let mut bytes = 0u64;
    let mut buffer = [0u8; 65536];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        bytes += count as u64;
        hash.update(&buffer[..count]);
    }
    let actual_hash = format!("{:x}", hash.finalize());
    let exit_passed = exit.map(i64::from) == recipe["expected_exit_code"].as_i64();
    let mut checks = vec![
        json!({"kind":"exit_code","expected":recipe["expected_exit_code"],"actual":exit,"passed":exit_passed}),
    ];
    if let Some(expected) = recipe["expected_stdout_sha256"].as_str() {
        checks.push(json!({"kind":"stdout_sha256","expected":expected,"actual":actual_hash,"passed":expected==actual_hash,"comparison":"exact stdout bytes; no newline or whitespace normalization"}));
    }
    let passed = !canceled && !timed_out && checks.iter().all(|c| c["passed"] == true);
    Ok(
        json!({"passed":passed,"expected_exit_code":recipe["expected_exit_code"],"actual_exit_code":exit,"validation":if checks.len()>1{"exit_code_and_stdout"}else{"exit_code_only"},"checks":checks,"stdout_sha256":actual_hash,"stdout_bytes":bytes,"canceled":canceled,"timed_out":timed_out}),
    )
}
