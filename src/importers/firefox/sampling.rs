//! Per-sample stack availability, separate from native mapping completeness.
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::collections::HashMap;

type Key = (u32, u32, u64);
pub(super) const LEGACY_DIAGNOSTIC: &str = "Legacy Windows profile has no exact per-sample stack-association metadata; caller chains remain unverified and can contain deferred StackWalk reuse. Reimporting a processed profile does not reconstruct its original ETL events";

#[derive(Default)]
pub(super) struct Evidence {
    events: HashMap<Key, Vec<Value>>,
    profile_counts: HashMap<Key, usize>,
}

fn os_id(value: &Value) -> Option<u32> {
    value
        .as_u64()
        .or_else(|| value.as_str()?.split('.').next()?.parse().ok())
        .and_then(|id| id.try_into().ok())
}

fn key(pid: &Value, tid: &Value, time: &Value) -> Result<Key> {
    let time = time.as_f64().context("sampling timestamp missing")?;
    ensure!(
        time.is_finite() && time >= 0.0,
        "invalid sampling timestamp"
    );
    Ok((
        os_id(pid).context("sampling OS process id missing")?,
        os_id(tid).context("sampling OS thread id missing")?,
        if time == 0.0 { 0 } else { time.to_bits() },
    ))
}

fn counter(value: &Value) -> Result<u64> {
    let text = value
        .as_str()
        .context("sampling raw timestamp must be a string")?;
    ensure!(
        !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit()),
        "invalid sampling raw timestamp"
    );
    text.parse().context("sampling raw timestamp exceeds u64")
}

fn address(value: &Value) -> Result<u64> {
    let text = value
        .as_str()
        .context("sampled instruction pointer missing")?;
    let hex = text
        .strip_prefix("0x")
        .context("invalid sampled instruction pointer")?;
    u64::from_str_radix(hex, 16).context("invalid sampled instruction pointer")
}

impl Evidence {
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    pub fn load(events: &[Value], raw: &Value) -> Result<Self> {
        let mut result = Self::default();
        for event in events
            .iter()
            .filter(|event| event["kind"] == "sampling" && event["event"] == "sample_stack")
        {
            let event_key = key(&event["pid"], &event["tid"], &event["time_ms"])?;
            let original = counter(&event["raw_timestamp"])?;
            let basis = event["stack_capture_basis"]
                .as_str()
                .context("sample stack basis missing")?;
            ensure!(
                [
                    "matched_user_stack",
                    "sampled_instruction_pointer_only",
                    "kernel_stack_only",
                    "missing_stack"
                ]
                .contains(&basis),
                "unsupported sample stack basis"
            );
            ensure!(
                ["on_cpu", "off_cpu"].contains(&event["sample_class"].as_str().unwrap_or("")),
                "invalid native sample class"
            );
            let depth = event["user_stack_depth"]
                .as_u64()
                .context("user stack depth missing")?;
            ensure!(depth <= 8192, "user stack depth exceeds read budget");
            let kernel = event["kernel_stack_available"]
                .as_bool()
                .context("kernel stack availability missing")?;
            ensure!(event["reason"].is_string(), "sample stack reason missing");
            if !event["instruction_pointer"].is_null() {
                address(&event["instruction_pointer"])?;
            }
            for field in ["stack_event_raw_timestamp", "stack_emitted_raw_timestamp"] {
                if !event[field].is_null() {
                    counter(&event[field])?;
                }
            }
            if basis == "matched_user_stack" {
                ensure!(depth > 0, "matched user stack is empty");
                ensure!(
                    counter(&event["stack_event_raw_timestamp"])? == original,
                    "user stack original timestamp does not match its sample"
                );
            } else {
                ensure!(depth == 0, "unavailable user stack has reported frames");
            }
            if basis == "sampled_instruction_pointer_only" {
                address(&event["instruction_pointer"])?;
                ensure!(
                    event["sample_class"] == "on_cpu",
                    "instruction-pointer fallback requires an on-CPU sample"
                );
            }
            if basis == "kernel_stack_only" {
                ensure!(kernel, "kernel-only sample has no kernel stack");
            }
            result
                .events
                .entry(event_key)
                .or_default()
                .push(event.clone());
        }
        if !result.events.is_empty() {
            result.count_profile(raw);
        }
        Ok(result)
    }

    fn count_profile(&mut self, raw: &Value) {
        for thread in raw["threads"].as_array().into_iter().flatten() {
            for (index, time) in thread["samples"]["time"]
                .as_array()
                .into_iter()
                .flatten()
                .enumerate()
            {
                if thread["samples"]["stack"][index].is_null()
                    || thread["samples"]["weight"][index]
                        .as_f64()
                        .is_some_and(|weight| weight <= 0.0)
                {
                    continue;
                }
                if let Ok(key) = key(&thread["pid"], &thread["tid"], time) {
                    *self.profile_counts.entry(key).or_default() += 1;
                }
            }
        }
        for child in raw["processes"].as_array().into_iter().flatten() {
            self.count_profile(child);
        }
    }

    pub fn annotate(
        &self,
        thread: &Value,
        time_ms: f64,
        record: &mut Value,
        mapping: Option<&Value>,
    ) -> Result<()> {
        if self.events.is_empty() {
            return Ok(());
        }
        let key = key(&thread["pid"], &thread["tid"], &json!(time_ms))?;
        let events = self.events.get(&key);
        if events.is_none()
            || events.is_some_and(|events| events.len() != 1)
            || self.profile_counts.get(&key) != Some(&1)
        {
            let status = if events.is_some() {
                "ambiguous"
            } else {
                "unavailable"
            };
            record["attributes"]["sampling_metadata_status"] = json!(status);
            record["attributes"]["sample_stack_basis"] = json!(status);
            record["attributes"]["user_stack_available"] = Value::Null;
            record["attributes"]["caller_stack_available"] = Value::Null;
            return Ok(());
        }
        let event = &events.unwrap()[0];
        let matched = event["stack_capture_basis"] == "matched_user_stack";
        let attributes = record["attributes"]
            .as_object_mut()
            .context("native sample attributes missing")?;
        attributes.insert("sampling_metadata_status".into(), json!("matched"));
        attributes.insert(
            "sample_stack_basis".into(),
            event["stack_capture_basis"].clone(),
        );
        attributes.insert("user_stack_available".into(), json!(matched));
        attributes.insert(
            "caller_stack_available".into(),
            json!(matched && event["user_stack_depth"].as_u64().unwrap() > 1),
        );
        for (output, input) in [
            ("original_sample_timestamp", "raw_timestamp"),
            ("sampled_instruction_pointer", "instruction_pointer"),
            ("sample_class", "sample_class"),
            ("user_stack_depth", "user_stack_depth"),
            ("kernel_stack_available", "kernel_stack_available"),
            ("stack_capture_reason", "reason"),
            ("stack_event_raw_timestamp", "stack_event_raw_timestamp"),
            ("stack_emitted_raw_timestamp", "stack_emitted_raw_timestamp"),
        ] {
            attributes.insert(output.into(), event[input].clone());
        }
        if event["stack_capture_basis"] == "sampled_instruction_pointer_only" {
            let original = address(&event["instruction_pointer"])?;
            let frames = mapping.and_then(|mapping| mapping["frames"].as_array());
            let status = if let Some(frames) = frames.filter(|frames| !frames.is_empty()) {
                ensure!(
                    frames.len() == 1 && address(&frames[0]["address"])? == original,
                    "instruction-pointer fallback disagrees with native frame evidence"
                );
                "verified"
            } else {
                "unavailable"
            };
            attributes.insert("instruction_pointer_frame_status".into(), json!(status));
        }
        Ok(())
    }

    pub fn diagnostics(&self) -> Vec<String> {
        let partial = self
            .events
            .values()
            .flatten()
            .filter(|event| {
                event["stack_capture_basis"] != "matched_user_stack"
                    || event["user_stack_depth"].as_u64().unwrap_or(0) <= 1
            })
            .count();
        let unavailable = self
            .profile_counts
            .iter()
            .filter(|(key, count)| {
                **count != 1 || self.events.get(key).is_none_or(|events| events.len() != 1)
            })
            .map(|(_, count)| count)
            .sum::<usize>();
        let mut diagnostics = Vec::new();
        if partial > 0 {
            diagnostics.push(format!("{partial} native sampling observations have an unavailable caller stack; original instruction-pointer, kernel-only or missing-stack evidence is partial and does not indicate module lifetime event loss"));
        }
        if unavailable > 0 {
            diagnostics.push(format!("{unavailable} processed native samples have unavailable or ambiguous exact stack-availability metadata; no nearest or future event was substituted"));
        }
        diagnostics
    }
}
