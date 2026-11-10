use hunin_core::{
    execution::PreparedProcess,
    project::{Project, Reference, TargetRef},
    query::{Query, QueryLimits},
    records::SourceHandle,
};
use hunin_platforms::{ProcessInstance, ResourceSnapshot, inspect_process, read_resources};
use serde_json::json;
use std::time::{Duration, Instant};

pub struct Resources {
    pub target: TargetRef,
    source: SourceHandle,
    instance: ProcessInstance,
    first: ResourceSnapshot,
    next: usize,
}

impl Resources {
    pub fn start(project: &Project, run: &Reference, pid: u32) -> Self {
        let instance = inspect_process(pid).unwrap();
        let first = read_resources(&instance).unwrap();
        let target = project
            .register_target(
                run,
                "target",
                serde_json::to_value(&instance).unwrap(),
                true,
            )
            .unwrap();
        let source = project
            .register_source(
                run,
                hunin_platforms::recording::resource_definition(
                    "resources",
                    target.clone(),
                    &first,
                ),
            )
            .unwrap();
        source
            .append(&[first.to_record("0", &target.id).unwrap()])
            .unwrap();
        source.ready(json!({"first_read":first.acquired,"scope":"process all threads excluding children"})).unwrap();
        Self {
            target,
            source,
            instance,
            first,
            next: 1,
        }
    }

    pub fn finish_work(&mut self, process: &mut PreparedProcess) -> (usize, u64) {
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut last;
        loop {
            assert!(
                Instant::now() < deadline,
                "prepared work did not finish under its deadline"
            );
            last = read_resources(&self.instance).unwrap();
            self.source
                .append(&[last
                    .to_record(&self.next.to_string(), &self.target.id)
                    .unwrap()])
                .unwrap();
            self.next += 1;
            if process.work_complete().unwrap() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let before = self.first.user_cpu.nanoseconds().unwrap()
            + self.first.system_cpu.nanoseconds().unwrap();
        let after = last.user_cpu.nanoseconds().unwrap() + last.system_cpu.nanoseconds().unwrap();
        assert!(
            after > before,
            "known CPU work must change process counters"
        );
        assert!(last.memory.iter().any(|memory| memory.bytes > 0));
        self.source
            .close(json!({"complete":true,"count":self.next}))
            .unwrap();
        process.finish().unwrap();
        (self.next, after - before)
    }
}

pub fn verify_reopened(project: &Project, run: &Reference, expected: usize) {
    let mut query = Query::new(run.clone());
    query.source = Some("resources".into());
    query.schema = Some("platform.process_resources".into());
    let mut cursor = None;
    let mut count = 0;
    loop {
        let page = project
            .query(&query, QueryLimits::default(), cursor.as_ref())
            .unwrap();
        for entry in page.records {
            let record = entry.record.unwrap();
            assert!(!record.payload["memory"].as_array().unwrap().is_empty());
            assert!(record.payload["user_cpu"]["denominator"].as_u64().unwrap() > 0);
            assert_eq!(record.id, count.to_string());
            count += 1;
        }
        cursor = page.continuation;
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(count, expected);
}
