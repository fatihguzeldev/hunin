//! Verify a compiler's printed Wasm body against retained module operators.
use super::wasm::Function;
use anyhow::{Context, Result};
use serde_json::Value;
use std::collections::BTreeSet;

fn hexes(text: &str) -> Result<Vec<u8>> {
    text.split([',', ' ', '\t'])
        .filter(|s| s.starts_with("0x"))
        .map(|token| {
            u8::from_str_radix(
                token.trim().trim_end_matches(',').trim_start_matches("0x"),
                16,
            )
            .context("invalid compiler source byte")
        })
        .collect()
}
fn normalized(text: &str) -> String {
    text.chars()
        .filter(char::is_ascii_alphanumeric)
        .flat_map(char::to_lowercase)
        .collect()
}
fn opcode_length(bytes: &[u8]) -> Option<usize> {
    match bytes.first()? {
        0xfb..=0xfe => {
            let mut at = 1;
            while at < bytes.len() && at < 6 {
                let b = bytes[at];
                at += 1;
                if b & 128 == 0 {
                    return Some(at);
                }
            }
            None
        }
        _ => Some(1),
    }
}
pub(super) fn source_matches(trace: &Value, function: &Function) -> Result<bool> {
    let (Some(source), Some(positions)) = (
        trace["source"].as_str(),
        trace["sourceLineToBytecodePosition"].as_array(),
    ) else {
        return Ok(false);
    };
    if positions
        .iter()
        .filter(|p| p.as_i64().is_some_and(|p| p >= 0))
        .count()
        != function.operations.len()
    {
        return Ok(false);
    }
    let mut prefix = vec![];
    let mut visited = BTreeSet::new();
    for (index, line) in source.lines().enumerate() {
        let code = line.split("//").next().unwrap_or("");
        let position = positions.get(index).and_then(Value::as_i64).unwrap_or(-1);
        if position < 0 {
            if visited.is_empty() {
                prefix.extend(hexes(code)?);
            }
            continue;
        }
        let start = position as u64;
        if !visited.insert(start) {
            return Ok(false);
        }
        let Some((end, opcode)) = function.operations.get(&start) else {
            return Ok(false);
        };
        let comment = line.split("//").nth(1).unwrap_or("").trim();
        let mnemonic = comment.split_whitespace().next().unwrap_or("");
        if normalized(mnemonic) != normalized(opcode) {
            return Ok(false);
        }
        let bytes = &function.body[start as usize..*end as usize];
        let Some(header) = opcode_length(bytes) else {
            return Ok(false);
        };
        if hexes(code)? != bytes[header..] {
            return Ok(false);
        }
    }
    let first = function
        .operations
        .first_key_value()
        .map(|(&offset, _)| offset as usize)
        .unwrap_or(function.body.len());
    Ok(visited.len() == function.operations.len() && prefix == function.body[..first])
}
