use serde::Serialize;
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};
use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};
mod retention;
pub(super) use retention::Retention;

const QUEUE_CAPACITY: usize = 4;

#[derive(Default)]
struct DeliveryCounters {
    generated_batches: AtomicU64,
    generated_rows: AtomicU64,
    empty_generated_batches: AtomicU64,
    queued_batches: AtomicU64,
    queued_rows: AtomicU64,
    queue_full_dropped_batches: AtomicU64,
    queue_full_dropped_rows: AtomicU64,
    receiver_closed_dropped_batches: AtomicU64,
    receiver_closed_dropped_rows: AtomicU64,
    stop_discarded_batches: AtomicU64,
    stop_discarded_rows: AtomicU64,
}
#[derive(Serialize)]
pub(super) struct Delivery {
    scope: &'static str,
    queue_capacity_batches: usize,
    generated_batches: u64,
    generated_rows: u64,
    empty_generated_batches: u64,
    queued_batches: u64,
    queued_rows: u64,
    queue_full_dropped_batches: u64,
    queue_full_dropped_rows: u64,
    receiver_closed_dropped_batches: u64,
    receiver_closed_dropped_rows: u64,
    stop_discarded_batches: u64,
    stop_discarded_rows: u64,
    os_observation_gaps: &'static str,
}
impl DeliveryCounters {
    fn snapshot(&self) -> Delivery {
        Delivery {
            scope: "generated_observation_rows; not unobserved CPU weights or OS coverage",
            queue_capacity_batches: QUEUE_CAPACITY,
            generated_batches: self.generated_batches.load(Ordering::Relaxed),
            generated_rows: self.generated_rows.load(Ordering::Relaxed),
            empty_generated_batches: self.empty_generated_batches.load(Ordering::Relaxed),
            queued_batches: self.queued_batches.load(Ordering::Relaxed),
            queued_rows: self.queued_rows.load(Ordering::Relaxed),
            queue_full_dropped_batches: self.queue_full_dropped_batches.load(Ordering::Relaxed),
            queue_full_dropped_rows: self.queue_full_dropped_rows.load(Ordering::Relaxed),
            receiver_closed_dropped_batches: self
                .receiver_closed_dropped_batches
                .load(Ordering::Relaxed),
            receiver_closed_dropped_rows: self.receiver_closed_dropped_rows.load(Ordering::Relaxed),
            stop_discarded_batches: self.stop_discarded_batches.load(Ordering::Relaxed),
            stop_discarded_rows: self.stop_discarded_rows.load(Ordering::Relaxed),
            os_observation_gaps: "unknown",
        }
    }
}
impl Delivery {
    pub fn discarded_rows(&self) -> u64 {
        self.queue_full_dropped_rows + self.receiver_closed_dropped_rows + self.stop_discarded_rows
    }
}
pub(super) struct Completion {
    pub delivery: Delivery,
    pub worker_error: Option<String>,
}

pub struct Observer {
    stopped: Arc<AtomicBool>,
    task: Option<tokio::task::JoinHandle<()>>,
    delivery: Arc<DeliveryCounters>,
}

impl Observer {
    pub fn start(
        root: u32,
        run_id: String,
        collector_wrapper: bool,
    ) -> (Self, tokio::sync::mpsc::Receiver<Vec<Value>>) {
        let stopped = Arc::new(AtomicBool::new(false));
        let worker_stopped = stopped.clone();
        let (sender, receiver) = tokio::sync::mpsc::channel(QUEUE_CAPACITY);
        let delivery = Arc::new(DeliveryCounters::default());
        let worker_delivery = delivery.clone();
        let task = tokio::task::spawn_blocking(move || {
            let mut system = System::new();
            let mut identities = ObservationIdentities::new(root);
            let mut sequence = 0;
            let mut previous = None;
            while !worker_stopped.load(Ordering::Acquire) {
                sequence += 1;
                let mut rows = observe(
                    &mut system,
                    &mut identities,
                    root,
                    &run_id,
                    sequence,
                    collector_wrapper,
                );
                let row_count = rows.len() as u64;
                worker_delivery
                    .generated_batches
                    .fetch_add(1, Ordering::Relaxed);
                worker_delivery
                    .generated_rows
                    .fetch_add(row_count, Ordering::Relaxed);
                if rows.is_empty() {
                    worker_delivery
                        .empty_generated_batches
                        .fetch_add(1, Ordering::Relaxed);
                }
                if worker_stopped.load(Ordering::Acquire) {
                    worker_delivery
                        .stop_discarded_batches
                        .fetch_add(1, Ordering::Relaxed);
                    worker_delivery
                        .stop_discarded_rows
                        .fetch_add(row_count, Ordering::Relaxed);
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
                match sender.try_send(rows) {
                    Ok(()) => {
                        worker_delivery
                            .queued_batches
                            .fetch_add(1, Ordering::Relaxed);
                        worker_delivery
                            .queued_rows
                            .fetch_add(row_count, Ordering::Relaxed);
                    }
                    Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                        worker_delivery
                            .queue_full_dropped_batches
                            .fetch_add(1, Ordering::Relaxed);
                        worker_delivery
                            .queue_full_dropped_rows
                            .fetch_add(row_count, Ordering::Relaxed);
                    }
                    Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                        worker_delivery
                            .receiver_closed_dropped_batches
                            .fetch_add(1, Ordering::Relaxed);
                        worker_delivery
                            .receiver_closed_dropped_rows
                            .fetch_add(row_count, Ordering::Relaxed);
                    }
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        });
        (
            Self {
                stopped,
                task: Some(task),
                delivery,
            },
            receiver,
        )
    }

    pub async fn finish(mut self) -> Completion {
        self.stopped.store(true, Ordering::Release);
        let worker_error = self
            .task
            .take()
            .unwrap()
            .await
            .err()
            .map(|error| error.to_string());
        Completion {
            delivery: self.delivery.snapshot(),
            worker_error,
        }
    }
}

impl Drop for Observer {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
    }
}

struct ObservationIdentities {
    root: Option<String>,
    tracked: HashSet<u32>,
}

impl ObservationIdentities {
    fn new(root: u32) -> Self {
        Self {
            root: super::process_identity::birth(root),
            tracked: HashSet::from([root]),
        }
    }
}

fn stable_birth<'a>(before: Option<&'a str>, after: Option<&str>) -> Option<&'a str> {
    before.filter(|value| Some(*value) == after)
}

fn owned_child_birth<'a>(parent: &str, child: Option<&'a str>) -> Option<&'a str> {
    child.filter(|child| super::process_identity::child_not_older_than_parent(parent, child))
}

fn observe(
    system: &mut System,
    identities: &mut ObservationIdentities,
    root: u32,
    run_id: &str,
    sequence: u64,
    collector_wrapper: bool,
) -> Vec<Value> {
    let refresh_started = std::time::Instant::now();
    let before = identities
        .tracked
        .iter()
        .filter_map(|pid| Some((*pid, super::process_identity::birth(*pid)?)))
        .collect::<HashMap<_, _>>();
    // sysinfo's cached lifetime check uses whole seconds. A fresh table avoids
    // retaining a prior process's resource counters after same-second PID reuse.
    *system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing()
            .without_tasks()
            .with_cpu()
            .with_memory(),
    );
    if stable_birth(
        identities.root.as_deref(),
        before.get(&root).map(String::as_str),
    )
    .is_none()
        || super::process_identity::birth(root).as_deref() != identities.root.as_deref()
    {
        return Vec::new();
    }
    let mut owned = HashMap::from([(Pid::from_u32(root), identities.root.clone().unwrap())]);
    let mut candidates = HashMap::new();
    loop {
        let previous = owned.len();
        for (pid, p) in system.processes() {
            if owned.contains_key(pid) {
                continue;
            }
            let Some(parent_birth) = p.parent().and_then(|parent| owned.get(&parent)) else {
                continue;
            };
            let birth = candidates
                .entry(*pid)
                .or_insert_with(|| super::process_identity::birth(pid.as_u32()));
            if let Some(birth) = owned_child_birth(parent_birth, birth.as_deref()) {
                owned.insert(*pid, birth.to_owned());
            }
        }
        if owned.len() == previous {
            break;
        }
    }
    let now = crate::model::now_us();
    let mut records = Vec::new();
    identities.tracked = owned.keys().map(|p| p.as_u32()).collect();
    for (pid, edge_birth) in owned {
        if let Some(p) = system.process(pid) {
            let after = super::process_identity::birth(pid.as_u32());
            let Some(birth) = stable_birth(
                before.get(&pid.as_u32()).map(String::as_str),
                after.as_deref(),
            ) else {
                continue;
            };
            if birth != edge_birth {
                continue;
            }
            let producer = format!("resources:{run_id}:{}:{birth}", pid.as_u32());
            let refresh_elapsed_us =
                refresh_started.elapsed().as_micros().min(u64::MAX as u128) as u64;
            for (name, value, unit) in [
                ("process.rss", p.memory() as f64, "bytes"),
                (
                    "process.cpu_time",
                    p.accumulated_cpu_time() as f64,
                    "milliseconds",
                ),
            ] {
                records.push(json!({"schema_version":1,"kind":"metric","id":format!("{producer}:{sequence}:{name}"),"run_id":run_id,"producer_id":producer,"pid":pid.as_u32(),"tid":"process","timestamp_us":now,"basis":"observed","name":name,"value":value,"unit":unit,"scope":"process","attributes":{"process_started_at_seconds":p.start_time(),"process_lifetime_id":birth,"requested_interval_us":100000,"observation_refresh_elapsed_us":refresh_elapsed_us,"temporality":if name=="process.rss" {"gauge"}else{"cumulative"},"process_role":if collector_wrapper&&pid.as_u32()==root{"collector"}else{"measured"},"process_name":p.name().to_string_lossy(),"lifetime_basis":"native_birth_identity_brackets_resource_refresh"}}));
            }
        }
    }
    records
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn actual_saturated_queue_reports_generated_row_delivery_loss() {
        let (observer, mut receiver) =
            Observer::start(std::process::id(), "queue-control".into(), false);
        tokio::time::timeout(Duration::from_secs(5), async {
            while receiver.capacity() > 0 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        tokio::time::sleep(Duration::from_millis(220)).await;
        let completion = observer.finish().await;
        assert!(completion.worker_error.is_none());
        let report = completion.delivery;
        assert!(report.queue_full_dropped_batches > 0);
        assert!(report.queue_full_dropped_rows > 0);
        let mut received_rows = 0;
        while let Ok(rows) = receiver.try_recv() {
            received_rows += rows.len() as u64;
        }
        assert_eq!(report.queued_rows, received_rows);
        assert_eq!(
            report.generated_rows,
            report.queued_rows + report.discarded_rows()
        );
        assert_eq!(
            report.generated_batches,
            report.queued_batches
                + report.queue_full_dropped_batches
                + report.receiver_closed_dropped_batches
                + report.stop_discarded_batches
        );
        assert_eq!(report.os_observation_gaps, "unknown");
    }

    #[test]
    fn reused_parent_pid_rejects_older_child_and_unknown_births() {
        // An orphan still names parent PID42, but that number now belongs to a
        // newer recording process. Stable child identity alone cannot own it.
        assert_eq!(
            owned_child_birth(
                "windows-creation-100ns:200",
                Some("windows-creation-100ns:100")
            ),
            None
        );
        assert_eq!(
            owned_child_birth(
                "windows-creation-100ns:200",
                Some("windows-creation-100ns:201")
            ),
            Some("windows-creation-100ns:201")
        );
        assert_eq!(
            owned_child_birth("linux-start-ticks:20", Some("linux-start-ticks:20")),
            Some("linux-start-ticks:20")
        );
        assert_eq!(
            owned_child_birth("macos-start-us:2:1", Some("macos-start-us:2:0")),
            None
        );
        assert_eq!(
            owned_child_birth("unknown", Some("windows-creation-100ns:201")),
            None
        );
        assert_eq!(owned_child_birth("windows-creation-100ns:200", None), None);
        assert_eq!(
            owned_child_birth("windows-creation-100ns:200", Some("linux-start-ticks:201")),
            None
        );
    }

    #[test]
    fn real_resource_rows_retain_native_birth_and_reject_a_replaced_root() {
        let pid = std::process::id();
        let mut system = System::new();
        let mut identities = ObservationIdentities::new(pid);
        let rows = observe(&mut system, &mut identities, pid, "run", 1, false);
        assert_eq!(rows.len(), 2);
        assert!(
            rows.iter()
                .all(|row| row["attributes"]["process_lifetime_id"].as_str()
                    == identities.root.as_deref())
        );
        // A numeric PID that now resolves to another birth must not seed tree
        // discovery or return that unrelated process's resources.
        identities.root = Some("previous-process-birth".into());
        assert!(observe(&mut system, &mut identities, pid, "run", 2, false).is_empty());
        identities.root = None;
        assert!(observe(&mut system, &mut identities, pid, "run", 3, false).is_empty());
    }
}
