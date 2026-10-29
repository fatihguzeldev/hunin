//! Mapping-instance evidence retained before Samply converts absolute stacks.
use super::checked_us;
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap},
    fs,
    io::Read,
    path::Path,
};

const UPSTREAM: &str = "da75c28f367454c621e690eeb4e44ec2ebb29a78";
pub(super) const MAX_SIDECAR: u64 = 160 * 1024 * 1024;

#[derive(Default)]
pub(super) struct Timeline {
    collector: Value,
    target_outcome: Option<Value>,
    epoch_ms: f64,
    instances: Vec<Instance>,
    samples: HashMap<String, Value>,
    lifecycle: Vec<Value>,
    snapshots: HashMap<String, Value>,
    image_events: HashMap<String, Value>,
    instances_by_id: HashMap<String, Vec<usize>>,
    complete: bool,
    diagnostics: Vec<String>,
    sampling: super::native_sampling::Evidence,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn external_move_and_resize_close_old_ranges_before_linking_boundary_sample() {
        let directory = tempfile::tempdir().unwrap();
        let raw = json!({"meta":{"startTime":1700000000000u64}});
        let raw_sha = format!("{:x}", Sha256::digest(serde_json::to_vec(&raw).unwrap()));
        let process = json!({"key":"p0","pid":"7","start_time_ms":0,"end_time_ms":40});
        let add = |time, id, name, start, end| json!({"kind":"mapping","op":"add","time_ms":time,"process":process,"load_id":id,"library":{"name":name},"start_avma":start,"end_avma":end,"relative_address_at_start":0});
        let movement = |time, old, start, end| json!({"kind":"mapping","op":"move","time_ms":time,"process":process,"old_start_avma":old,"start_avma":start,"end_avma":end});
        let input = json!({"schema_version":1,"raw_profile_sha256":raw_sha,"epoch_start_ms":1700000000000u64,"truncated":false,"collector":{"upstream_commit":UPSTREAM,"revision":"hunin-native1","os":"linux"},"events":[
            add(10,"1","A","0x1000","0x1800"),add(11,"2","B","0x2000","0x2800"),
            movement(20,"0x1000","0x2000","0x2800"),movement(30,"0x2000","0x2000","0x2900"),
            {"kind":"sample","thread":{"process":process,"tid":"8"},"time_ms":30,"mapping_time_ms":30,"frames":[{"load_id":"1","lookup_address":"0x2100","relative_address":256}]},
            {"kind":"sample","thread":{"process":process,"tid":"8"},"time_ms":29.9995,"mapping_time_ms":29.9995,"frames":[{"load_id":"1","lookup_address":"0x2100","relative_address":256}]}
        ]});
        let path = directory.path().join("profile.lifetimes.json");
        let bytes = serde_json::to_vec(&input).unwrap();
        fs::write(&path, &bytes).unwrap();
        let sha = format!("{:x}", Sha256::digest(&bytes));
        let timeline = Timeline::load(&path, &raw_sha, Some(&sha), &raw)
            .unwrap()
            .unwrap();
        let mut sample = json!({"id":"sample","run_id":"run","producer_id":"target","pid":7,"tid":"8","timestamp_us":1700000000030000u64,"attributes":{}});
        let links = timeline
            .link_sample(&json!({"pid":"7","tid":"8"}), 30.0, &mut sample, "key")
            .unwrap();
        assert_eq!(links.len(), 1);
        assert!(links[0]["to"].as_str().unwrap().ends_with(":g2"));
        let before = timeline
            .link_sample(&json!({"pid":"7","tid":"8"}), 29.9995, &mut sample, "key")
            .unwrap();
        assert!(before[0]["to"].as_str().unwrap().ends_with(":g1"));
        let scopes = timeline.records("run", "key").unwrap();
        let displaced = scopes
            .iter()
            .find(|r| r["attributes"]["module"] == "B")
            .unwrap();
        assert_eq!(
            displaced["duration_us"], 9000,
            "move must close the mapping displaced at its new address"
        );
    }
}
#[derive(Clone)]
struct Instance {
    process: Value,
    load_id: String,
    generation: u64,
    library: Value,
    start: f64,
    end: Option<f64>,
    start_avma: u64,
    end_avma: u64,
    relative_start: u64,
    start_basis: String,
    end_basis: String,
    start_window: Value,
    end_window: Value,
    end_reason: String,
    start_censored: bool,
    end_censored: bool,
}
fn ms(value: &Value) -> Result<f64> {
    let n = value
        .as_f64()
        .context("native lifetime timestamp missing")?;
    ensure!(
        n.is_finite() && n >= 0.0,
        "invalid native lifetime timestamp"
    );
    Ok(n)
}
fn address(value: &Value) -> Result<u64> {
    u64::from_str_radix(
        value
            .as_str()
            .context("native address missing")?
            .trim_start_matches("0x"),
        16,
    )
    .context("invalid native address")
}
fn sample_key(pid: &str, tid: &str, time: f64) -> String {
    format!("{pid}:{tid}:{time:.6}")
}
fn os_pid(value: &Value) -> Result<u64> {
    value
        .as_str()
        .context("native process pid missing")?
        .split('.')
        .next()
        .unwrap()
        .parse()
        .context("invalid native process pid")
}

impl Timeline {
    pub fn load(
        path: &Path,
        raw_sha: &str,
        expected_sha: Option<&str>,
        raw: &Value,
    ) -> Result<Option<Self>> {
        if !path.is_file() {
            return Ok(None);
        }
        ensure!(
            fs::metadata(path)?.len() <= MAX_SIDECAR,
            "native lifetime sidecar exceeds read budget"
        );
        let bytes = fs::read(path)?;
        ensure!(
            expected_sha == Some(format!("{:x}", Sha256::digest(&bytes)).as_str()),
            "native lifetime sidecar changed while importing"
        );
        let mut decoded = Vec::new();
        let bytes = if bytes.starts_with(&[0x1f, 0x8b]) {
            flate2::read::GzDecoder::new(bytes.as_slice())
                .take(MAX_SIDECAR + 1)
                .read_to_end(&mut decoded)?;
            ensure!(
                decoded.len() as u64 <= MAX_SIDECAR,
                "native lifetime sidecar exceeds decompressed budget"
            );
            &decoded
        } else {
            &bytes
        };
        let input: Value = serde_json::from_slice(bytes)?;
        ensure!(
            input["schema_version"] == 1 && input["raw_profile_sha256"] == raw_sha,
            "native lifetime sidecar does not match raw profile"
        );
        ensure!(
            input["collector"]["upstream_commit"] == UPSTREAM
                && input["collector"]["revision"] == "hunin-native1",
            "unsupported native lifetime producer"
        );
        let epoch = ms(&input["epoch_start_ms"])?;
        ensure!(
            (epoch - ms(&raw["meta"]["startTime"])?).abs() < 0.001,
            "native lifetime clock does not match raw profile"
        );
        let events = input["events"]
            .as_array()
            .context("native lifetime events missing")?;
        let mut result = Self {
            collector: input["collector"].clone(),
            epoch_ms: epoch,
            complete: input["truncated"] == false,
            sampling: super::native_sampling::Evidence::load(events, raw)?,
            ..Self::default()
        };
        if let Some(outcome) = input.get("target_outcome").filter(|v| !v.is_null()) {
            let state = outcome["state"]
                .as_str()
                .context("native target outcome state missing")?;
            ensure!(
                ["exited", "signaled", "running", "unknown", "not_launched"].contains(&state),
                "invalid native target outcome state"
            );
            ensure!(
                outcome["source"] == "root_target_os_wait_status",
                "unsupported native target outcome source"
            );
            if matches!(state, "exited" | "signaled") {
                ensure!(
                    outcome["pid"].as_u64().is_some_and(|pid| pid > 0),
                    "native target outcome pid missing"
                );
                let wait = outcome["wait_observed_at_us"]
                    .as_u64()
                    .context("native wait observation time missing")?;
                ensure!(
                    wait <= 9_007_199_254_740_991,
                    "native wait timestamp exceeds safe integer range"
                );
                if let Some(launch) = outcome["launch_observed_at_us"].as_u64() {
                    ensure!(launch <= wait, "native wait precedes launch observation");
                }
                if state == "exited" {
                    ensure!(
                        outcome["exit_code"]
                            .as_i64()
                            .is_some_and(|v| i32::try_from(v).is_ok())
                            && outcome["signal"].is_null(),
                        "invalid native exit status"
                    );
                } else {
                    ensure!(
                        outcome["signal"].as_u64().is_some_and(|v| v > 0)
                            && outcome["exit_code"].is_null(),
                        "invalid native signal status"
                    );
                }
            }
            result.target_outcome = Some(outcome.clone());
        }
        for event in events {
            if event["kind"] == "loss" || event["event"] == "events_lost" {
                result.complete = false;
            }
            if event["kind"] == "lifecycle" {
                result.lifecycle.push(event.clone());
                if event["event"]
                    .as_str()
                    .is_some_and(|name| name.starts_with("image_"))
                {
                    let key = format!(
                        "{}:{}:{}",
                        event["process"]["key"], event["time_ms"], event["data"]["start_avma"]
                    );
                    result.image_events.insert(key, event.clone());
                }
            }
            if event["kind"] == "snapshot" {
                let key = format!("{}:{}", event["process"]["key"], event["time_ms"]);
                result.snapshots.insert(key, event.clone());
            }
            if event["kind"] == "sample" {
                let thread = &event["thread"];
                let key = sample_key(
                    thread["process"]["pid"]
                        .as_str()
                        .context("sample process missing")?,
                    thread["tid"].as_str().context("sample thread missing")?,
                    ms(&event["time_ms"])?,
                );
                ensure!(
                    result.samples.insert(key, event.clone()).is_none(),
                    "ambiguous native sample mapping link"
                );
            }
        }
        let mut ops: Vec<_> = events.iter().filter(|e| e["kind"] == "mapping").collect();
        ops.sort_by(|a, b| {
            a["time_ms"]
                .as_f64()
                .partial_cmp(&b["time_ms"].as_f64())
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let mut active: BTreeMap<(String, u64), usize> = BTreeMap::new();
        let mut load_ids = std::collections::HashSet::new();
        for op in ops {
            let process = &op["process"];
            let process_key = process["key"]
                .as_str()
                .context("native process instance missing")?
                .to_owned();
            os_pid(&process["pid"])?;
            let time = ms(&op["time_ms"])?;
            let (basis, window, censored) = result.boundary(process_key.as_str(), op, time);
            match op["op"].as_str().context("mapping operation missing")? {
                "add" => {
                    let start = address(&op["start_avma"])?;
                    let end = address(&op["end_avma"])?;
                    ensure!(end > start, "native mapping range is empty or reversed");
                    let overlaps: Vec<_> = active
                        .iter()
                        .filter(|((p, a), i)| {
                            p == &process_key && *a < end && start < result.instances[**i].end_avma
                        })
                        .map(|(key, _)| key.clone())
                        .collect();
                    for key in overlaps {
                        if let Some(i) = active.remove(&key) {
                            result.close(
                                i,
                                time,
                                "address_replaced",
                                &basis,
                                window.clone(),
                                true,
                            )?;
                        }
                    }
                    let load = op["load_id"]
                        .as_str()
                        .context("native load id missing")?
                        .to_owned();
                    ensure!(
                        load_ids.insert((process_key.clone(), load.clone())),
                        "duplicate native load instance"
                    );
                    let index = result.instances.len();
                    result.instances.push(Instance {
                        process: process.clone(),
                        load_id: load,
                        generation: 0,
                        library: op["library"].clone(),
                        start: time,
                        end: None,
                        start_avma: start,
                        end_avma: end,
                        relative_start: op["relative_address_at_start"]
                            .as_u64()
                            .context("native mapping relative base missing")?,
                        start_basis: basis,
                        end_basis: "right_censored".into(),
                        start_window: window,
                        end_window: json!([null, null]),
                        end_reason: "capture_or_process_boundary".into(),
                        start_censored: censored,
                        end_censored: true,
                    });
                    active.insert((process_key, start), index);
                }
                "remove" => {
                    if let Some(i) = active.remove(&(process_key, address(&op["start_avma"])?)) {
                        result.close(i, time, "unmapped", &basis, window, censored)?;
                    }
                }
                "clear" => {
                    let keys: Vec<_> = active
                        .keys()
                        .filter(|(p, _)| p == &process_key)
                        .cloned()
                        .collect();
                    for key in keys {
                        let i = active.remove(&key).unwrap();
                        result.close(
                            i,
                            time,
                            "address_space_cleared",
                            &basis,
                            window.clone(),
                            true,
                        )?;
                    }
                }
                "move" => {
                    let old_start = address(&op["old_start_avma"])?;
                    let index = active
                        .remove(&(process_key.clone(), old_start))
                        .context("moved native mapping has no prior address range")?;
                    result.close(
                        index,
                        time,
                        "mapping_moved",
                        "collector_mapping_move",
                        json!([time, time]),
                        false,
                    )?;
                    let mut next = result.instances[index].clone();
                    next.start = time;
                    next.end = None;
                    next.generation += 1;
                    next.start_avma = address(&op["start_avma"])?;
                    next.end_avma = address(&op["end_avma"])?;
                    ensure!(
                        next.end_avma > next.start_avma,
                        "moved mapping has invalid range"
                    );
                    let displaced: Vec<_> = active
                        .iter()
                        .filter(|((p, a), i)| {
                            p == &process_key
                                && *a < next.end_avma
                                && next.start_avma < result.instances[**i].end_avma
                        })
                        .map(|(key, _)| key.clone())
                        .collect();
                    for key in displaced {
                        let i = active.remove(&key).unwrap();
                        result.close(
                            i,
                            time,
                            "address_replaced",
                            "collector_mapping_move",
                            json!([null, time]),
                            true,
                        )?;
                    }
                    next.start_basis = "collector_mapping_move".into();
                    next.start_window = json!([time, time]);
                    next.start_censored = false;
                    next.end_basis = "right_censored".into();
                    next.end_window = json!([null, null]);
                    next.end_censored = true;
                    next.end_reason = "capture_or_process_boundary".into();
                    let new_index = result.instances.len();
                    active.insert((process_key, next.start_avma), new_index);
                    result.instances.push(next);
                }
                other => bail!("unsupported native mapping operation {other}"),
            }
        }
        for i in active.into_values() {
            if let Some(end) = result.instances[i].process["end_time_ms"].as_f64() {
                result.close(
                    i,
                    end,
                    "process_boundary",
                    "process_exit_observation",
                    json!([null, end]),
                    true,
                )?;
            }
        }
        for (index, instance) in result.instances.iter().enumerate() {
            result
                .instances_by_id
                .entry(format!(
                    "{}:{}",
                    instance.process["key"].as_str().unwrap(),
                    instance.load_id
                ))
                .or_default()
                .push(index);
        }
        if !result.complete {
            result.diagnostics.push("Native lifetime evidence is incomplete (lost events, unavailable snapshots or bounded sidecar truncation); intervals are collector observations, not proof of continuous mapping".into());
        }
        result.diagnostics.extend(result.sampling.diagnostics());
        if result.collector["os"] == "windows" && result.sampling.is_empty() {
            result
                .diagnostics
                .push(super::native_sampling::LEGACY_DIAGNOSTIC.into());
        }
        if result.collector["patch_sha256"].is_null() {
            result.diagnostics.push("Native lifetime collector has no build-time patch digest; retain its binary/source provenance before trusting a distribution identity".into());
        }
        Ok(Some(result))
    }

    fn boundary(&self, process: &str, op: &Value, time: f64) -> (String, Value, bool) {
        if self.collector["os"] == "windows" {
            let event = self.image_events.get(&format!(
                "{}:{}:{}",
                json!(process),
                op["time_ms"],
                op["start_avma"]
            ));
            let rundown = event.is_none_or(|e| e["event"] == "image_dcstart");
            return (
                if rundown {
                    "etw_rundown_or_unknown"
                } else {
                    "etw_image_event"
                }
                .into(),
                json!([if rundown { Value::Null } else { json!(time) }, time]),
                rundown,
            );
        }
        let snapshot = self
            .snapshots
            .get(&format!("{}:{}", json!(process), op["time_ms"]));
        if let Some(snapshot) = snapshot {
            let lower = snapshot["previous_observation_ms"].clone();
            let upper = snapshot
                .get("observation_completed_ms")
                .cloned()
                .unwrap_or(json!(time));
            return (
                snapshot["basis"].as_str().unwrap_or("snapshot").into(),
                json!([lower, upper]),
                lower.is_null(),
            );
        }
        (
            if self.collector["os"] == "linux" {
                "perf_mapping_event"
            } else {
                "initial_snapshot"
            }
            .into(),
            json!([null, time]),
            true,
        )
    }
    fn close(
        &mut self,
        index: usize,
        time: f64,
        reason: &str,
        basis: &str,
        window: Value,
        censored: bool,
    ) -> Result<()> {
        let instance = &mut self.instances[index];
        ensure!(
            time >= instance.start,
            "native mapping ends before it starts"
        );
        instance.end = Some(time);
        instance.end_reason = reason.into();
        instance.end_basis = basis.into();
        instance.end_window = window;
        instance.end_censored = censored;
        Ok(())
    }
    pub fn thread_basis(&self, thread: &Value, record: &mut Value) {
        let attrs = &mut record["attributes"];
        if self.collector["os"] == "macos" {
            attrs["boundary_basis"] = json!("task_thread_enumeration_observation");
            attrs["start_basis"] = json!("first_enumerated_or_capture_start");
            attrs["end_basis"] = json!(if thread["unregisterTime"].is_null() {
                "right_censored"
            } else {
                "thread_death_or_process_observation"
            });
            return;
        }
        let pid = thread["pid"]
            .as_str()
            .unwrap_or("")
            .split('.')
            .next()
            .and_then(|v| v.parse::<u64>().ok());
        let tid = thread["tid"]
            .as_str()
            .unwrap_or("")
            .split('.')
            .next()
            .and_then(|v| v.parse::<u64>().ok());
        let mut exact = 0;
        for (field, boundary, names) in [
            ("registerTime", "start_basis", ["thread_start", "perf_fork"]),
            ("unregisterTime", "end_basis", ["thread_end", "perf_exit"]),
        ] {
            let event = self.lifecycle.iter().find(|e| {
                e["pid"].as_u64() == pid
                    && e["tid"].as_u64() == tid
                    && names.iter().any(|name| e["event"] == *name)
                    && e["time_ms"]
                        .as_f64()
                        .zip(thread[field].as_f64())
                        .is_some_and(|(a, b)| (a - b).abs() < 0.001)
            });
            if let Some(event) = event {
                attrs[boundary] = event["event"].clone();
                exact += 1;
            }
        }
        attrs["exact_event_timestamps"] = json!(exact == 2 && self.complete);
        attrs["event_evidence_complete"] = json!(self.complete);
    }
    pub fn libraries(&self) -> impl Iterator<Item = &Value> {
        self.instances.iter().map(|i| &i.library)
    }
    pub fn diagnostics(&self) -> Vec<String> {
        self.diagnostics.clone()
    }
    fn id(instance: &Instance, key: &str) -> String {
        format!(
            "profile:{key}:image:{}:{}:g{}",
            instance.process["key"].as_str().unwrap(),
            instance.load_id,
            instance.generation
        )
    }
    pub fn records(&self, run: &str, key: &str) -> Result<Vec<Value>> {
        let records = self.instances.iter().map(|i| {
            let pid=os_pid(&i.process["pid"])?;
            let process_start=ms(&i.process["start_time_ms"])?;
            if i.end.is_some_and(|end|end<process_start) { return Ok(None); }
            let start=i.start.max(process_start);
            let lifetime=ms(&i.process["start_time_ms"])?;
            let mut record=json!({"schema_version":1,"kind":if i.end.is_some(){"span"}else{"event"},"id":Self::id(i,key),"run_id":run,"producer_id":format!("profile:{key}:{pid}:{lifetime}"),"pid":pid,"tid":"process","timestamp_us":checked_us((self.epoch_ms+start)*1000.0,"native module time")?,"basis":"observed","name":"module.observed_lifetime","attributes":{
                "source_format":"samply-native-lifetimes","scope":"module","module":i.library["name"],"library":i.library,"mapping_instance_id":format!("{}:g{}",i.load_id,i.generation),"collector_load_id":i.load_id,"process_instance_id":i.process["key"],"start_avma":format!("0x{:x}",i.start_avma),"end_avma":format!("0x{:x}",i.end_avma),"boundary_basis":{"start":i.start_basis,"end":i.end_basis},"start_window_ms":i.start_window,"end_window_ms":i.end_window,"inherited_at_process_start":i.start<process_start,"start_censored":i.start_censored || i.start<process_start,"end_censored":i.end_censored,"end_reason":i.end_reason,"exact_event_timestamps":i.start_basis=="etw_image_event" && i.end_basis=="etw_image_event","evidence_complete":self.complete,"collector":self.collector,"clock":"unix_epoch","clock_alignment":"profile_start_epoch_plus_relative_time"
            }});
            if let Some(end)=i.end { record["duration_us"]=json!(checked_us((end-start)*1000.0,"native module duration")?); }
            Ok(Some(record))
        }).collect::<Result<Vec<_>>>()?;
        let mut records: Vec<_> = records.into_iter().flatten().collect();
        if let Some(outcome) = &self.target_outcome
            && (outcome["state"] == "exited" || outcome["state"] == "signaled")
        {
            let pid = outcome["pid"].as_u64().unwrap();
            let process = self
                .instances
                .iter()
                .find(|i| i.process["pid"].as_str() == Some(pid.to_string().as_str()))
                .map(|i| &i.process);
            let producer = if let Some(process) = process {
                format!("profile:{key}:{pid}:{}", ms(&process["start_time_ms"])?)
            } else {
                format!("profile:{key}:root-target:{pid}")
            };
            records.push(json!({"schema_version":1,"kind":"event","id":format!("profile:{key}:root-target-outcome"),"run_id":run,"producer_id":producer,"pid":pid,"tid":"process","timestamp_us":outcome["wait_observed_at_us"],"basis":"observed","name":"process.wait_outcome","attributes":{"scope":"process","source_format":"samply-native-lifetimes","process_id_source":"root_target_os_wait_status","process_instance_id":process.map(|p|&p["key"]),"target_outcome":outcome,"clock":"unix_epoch","clock_alignment":"os_wait_wall_clock_observation","timestamp_semantics":"time the collector observed OS wait status; not exact process death time"}}));
        }
        Ok(records)
    }

    pub fn link_sample(
        &self,
        thread: &Value,
        time: f64,
        record: &mut Value,
        key: &str,
    ) -> Result<Vec<Value>> {
        let sample = self.samples.get(&sample_key(
            thread["pid"]
                .as_str()
                .context("native sample pid missing")?,
            thread["tid"]
                .as_str()
                .context("native sample tid missing")?,
            time,
        ));
        self.sampling.annotate(thread, time, record, sample)?;
        let Some(sample) = sample else {
            record["attributes"]["native_mapping_status"] = json!("sample_link_unavailable");
            return Ok(Vec::new());
        };
        let process = sample["thread"]["process"]["key"]
            .as_str()
            .context("sample mapping process missing")?;
        let mapping_time = ms(&sample["mapping_time_ms"])?;
        let mut links = Vec::new();
        let mut relations = Vec::new();
        let mut linked = std::collections::HashSet::new();
        for frame in sample["frames"]
            .as_array()
            .context("sample mapping frames missing")?
        {
            let mut link = frame.clone();
            let Some(load) = frame["load_id"].as_str() else {
                link["record_id"] = Value::Null;
                link["reason"] = json!("unmapped_or_kernel_frame");
                links.push(link);
                continue;
            };
            let lookup = address(&frame["lookup_address"])?;
            let indices = self
                .instances_by_id
                .get(&format!("{process}:{load}"))
                .context("sample references missing native load instance")?;
            let matches: Vec<_> = indices
                .iter()
                .map(|i| &self.instances[*i])
                .filter(|i| {
                    // These times share the same collector conversion; widening either
                    // bound can select two generations around a move or resize.
                    lookup >= i.start_avma
                        && lookup < i.end_avma
                        && mapping_time >= i.start
                        && i.end.is_none_or(|end| mapping_time < end)
                })
                .collect();
            ensure!(
                matches.len() == 1,
                "sample has missing or ambiguous native mapping generation"
            );
            let instance = matches[0];
            ensure!(
                lookup >= instance.start_avma && lookup < instance.end_avma,
                "sample address outside native mapping"
            );
            ensure!(
                frame["relative_address"].as_u64()
                    == Some(lookup - instance.start_avma + instance.relative_start),
                "sample relative address disagrees with mapping"
            );
            ensure!(
                mapping_time >= instance.start && instance.end.is_none_or(|end| mapping_time < end),
                "sample mapping lies outside its load lifetime"
            );
            let id = Self::id(instance, key);
            link["record_id"] = json!(id);
            links.push(link);
            if linked.insert(id.clone()) {
                relations.push(json!({"schema_version":1,"kind":"relation","id":format!("{}:native-map:{}",record["id"].as_str().unwrap(),relations.len()),"run_id":record["run_id"],"producer_id":record["producer_id"],"pid":record["pid"],"tid":record["tid"],"timestamp_us":record["timestamp_us"],"basis":"observed","from":record["id"],"to":id,"relation":"sampled_in_code_instance","evidence":[record["id"],id],"attributes":{"source_format":"samply-native-lifetimes","mapping_time_ms":mapping_time,"evidence_complete":self.complete}}));
            }
        }
        record["attributes"]["native_load_instances"] = json!(links);
        Ok(relations)
    }
}
