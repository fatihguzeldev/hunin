use super::StartRequest;
use anyhow::{Result, bail};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::path::Path;

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
    maximum_bytes: u64,
    checkpoint: impl FnMut() -> Result<()>,
) -> Result<Value> {
    let mut hash = Sha256::new();
    let mut bytes = 0u64;
    crate::host::visit_regular(stdout, maximum_bytes, &mut bytes, checkpoint, |chunk| {
        hash.update(chunk);
        Ok(())
    })?;
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

/// An unavailable digest is not a mismatch. Exit-only recipes can still be
/// evaluated; a requested exact-output oracle stays unknown without full bytes.
pub(super) fn unavailable(
    recipe: &Value,
    exit: Option<i32>,
    canceled: bool,
    timed_out: bool,
    error: &anyhow::Error,
) -> Value {
    let exit_passed = exit.map(i64::from) == recipe["expected_exit_code"].as_i64();
    let mut checks = vec![
        json!({"kind":"exit_code","expected":recipe["expected_exit_code"],"actual":exit,"passed":exit_passed}),
    ];
    let requested = recipe["expected_stdout_sha256"].is_string();
    if requested {
        checks.push(json!({"kind":"stdout_sha256","expected":recipe["expected_stdout_sha256"],"actual":null,"passed":null,"comparison":"exact stdout bytes; no newline or whitespace normalization"}));
    }
    let passed = if canceled || timed_out || !exit_passed {
        json!(false)
    } else if requested {
        Value::Null
    } else {
        json!(true)
    };
    json!({
        "passed": passed,
        "expected_exit_code": recipe["expected_exit_code"],
        "actual_exit_code": exit,
        "validation": if requested {"exit_code_and_stdout"} else {"exit_code_only"},
        "checks": checks,
        "stdout_sha256": null,
        "stdout_bytes": null,
        "stdout_digest_status": "unavailable",
        "stdout_digest_diagnostic": format!("{error:#}"),
        "canceled": canceled,
        "timed_out": timed_out,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounded_stdout_oracle_keeps_exact_success_and_unknown_distinct_from_mismatch() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("stdout.txt");
        std::fs::write(&path, b"exact\n").unwrap();
        let recipe = json!({"expected_exit_code":0,"expected_stdout_sha256":format!("{:x}",Sha256::digest(b"exact\n"))});
        let exact = evaluate(&recipe, &path, Some(0), false, false, 6, || Ok(())).unwrap();
        assert_eq!(exact["passed"], true);
        assert_eq!(exact["stdout_bytes"], 6);
        let error = evaluate(&recipe, &path, Some(0), false, false, 5, || Ok(())).unwrap_err();
        assert!(error.is::<crate::host::FileReadLimit>());
        let unknown = unavailable(&recipe, Some(0), false, false, &error);
        assert!(unknown["passed"].is_null());
        assert!(unknown["checks"][1]["passed"].is_null());
        assert_eq!(unknown["checks"][0]["passed"], true);
        assert_eq!(
            unavailable(
                &json!({"expected_exit_code":0}),
                Some(0),
                false,
                false,
                &error
            )["passed"],
            true
        );
        assert_eq!(
            unavailable(&recipe, Some(1), false, false, &error)["passed"],
            false
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"exact\n");
        let error = evaluate(&recipe, &path, Some(0), false, false, 6, || {
            anyhow::bail!("expired finishing checkpoint")
        })
        .unwrap_err();
        assert!(unavailable(&recipe, Some(0), false, false, &error)["passed"].is_null());
    }
}
