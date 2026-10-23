use serde_json::{Value, json};
use std::collections::HashSet;
use sysinfo::{Pid, ProcessesToUpdate, System};

pub fn observe(
    system: &mut System,
    root: u32,
    run_id: &str,
    sequence: u64,
    collector_wrapper: bool,
) -> Vec<Value> {
    system.refresh_processes(ProcessesToUpdate::All, true);
    let mut owned = HashSet::from([Pid::from_u32(root)]);
    loop {
        let previous = owned.len();
        for (pid, p) in system.processes() {
            if p.parent().is_some_and(|parent| owned.contains(&parent)) {
                owned.insert(*pid);
            }
        }
        if owned.len() == previous {
            break;
        }
    }
    let now = crate::model::now_us();
    let mut records = Vec::new();
    for pid in owned {
        if let Some(p) = system.process(pid) {
            let producer = format!("resources:{run_id}:{}:{}", pid.as_u32(), p.start_time());
            for (name, value, unit) in [
                ("process.rss", p.memory() as f64, "bytes"),
                (
                    "process.cpu_time",
                    p.accumulated_cpu_time() as f64,
                    "milliseconds",
                ),
            ] {
                records.push(json!({"schema_version":1,"kind":"metric","id":format!("{producer}:{sequence}:{name}"),"run_id":run_id,"producer_id":producer,"pid":pid.as_u32(),"tid":"process","timestamp_us":now,"basis":"observed","name":name,"value":value,"unit":unit,"scope":"process","attributes":{"process_started_at_seconds":p.start_time(),"interval_us":100000,"temporality":if name=="process.rss" {"gauge"}else{"cumulative"},"process_role":if collector_wrapper&&pid.as_u32()==root{"collector"}else{"measured"},"process_name":p.name().to_string_lossy(),"lifetime_basis":"pid_and_os_start_time_seconds"}}));
            }
        }
    }
    records
}
