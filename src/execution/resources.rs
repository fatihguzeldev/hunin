use serde_json::{Value, json};
use std::{
    collections::HashSet,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

pub struct Observer {
    stopped: Arc<AtomicBool>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl Observer {
    pub fn start(
        root: u32,
        run_id: String,
        collector_wrapper: bool,
    ) -> (Self, tokio::sync::mpsc::Receiver<Vec<Value>>) {
        let stopped = Arc::new(AtomicBool::new(false));
        let worker_stopped = stopped.clone();
        let (sender, receiver) = tokio::sync::mpsc::channel(4);
        let task = tokio::task::spawn_blocking(move || {
            let mut system = System::new();
            let mut sequence = 0;
            let mut previous = None;
            while !worker_stopped.load(Ordering::Acquire) {
                sequence += 1;
                let mut rows = observe(&mut system, root, &run_id, sequence, collector_wrapper);
                if worker_stopped.load(Ordering::Acquire) {
                    break;
                }
                if let Some(timestamp) = rows.first().and_then(|r| r["timestamp_us"].as_u64()) {
                    for row in &mut rows {
                        row["attributes"]["interval_us"] =
                            json!(previous.map(|last: u64| timestamp.saturating_sub(last)));
                    }
                    previous = Some(timestamp);
                }
                // A slow consumer must not block process termination. Missing
                // observations remain missing rather than replayed as fresh data.
                let _ = sender.try_send(rows);
                std::thread::sleep(Duration::from_millis(100));
            }
        });
        (
            Self {
                stopped,
                task: Some(task),
            },
            receiver,
        )
    }

    pub async fn finish(mut self) -> Result<(), tokio::task::JoinError> {
        self.stopped.store(true, Ordering::Release);
        self.task.take().unwrap().await
    }
}

impl Drop for Observer {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
    }
}

pub fn observe(
    system: &mut System,
    root: u32,
    run_id: &str,
    sequence: u64,
    collector_wrapper: bool,
) -> Vec<Value> {
    let refresh_started = std::time::Instant::now();
    system.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing()
            .without_tasks()
            .with_cpu()
            .with_memory(),
    );
    let refresh_elapsed_us = refresh_started.elapsed().as_micros().min(u64::MAX as u128) as u64;
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
                records.push(json!({"schema_version":1,"kind":"metric","id":format!("{producer}:{sequence}:{name}"),"run_id":run_id,"producer_id":producer,"pid":pid.as_u32(),"tid":"process","timestamp_us":now,"basis":"observed","name":name,"value":value,"unit":unit,"scope":"process","attributes":{"process_started_at_seconds":p.start_time(),"requested_interval_us":100000,"observation_refresh_elapsed_us":refresh_elapsed_us,"temporality":if name=="process.rss" {"gauge"}else{"cumulative"},"process_role":if collector_wrapper&&pid.as_u32()==root{"collector"}else{"measured"},"process_name":p.name().to_string_lossy(),"lifetime_basis":"pid_and_os_start_time_seconds"}}));
            }
        }
    }
    records
}
