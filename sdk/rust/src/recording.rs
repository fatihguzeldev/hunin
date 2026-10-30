use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    sync::{
        atomic::{AtomicU32, AtomicU64, Ordering},
        Mutex, OnceLock,
    },
    time::{Instant, SystemTime, UNIX_EPOCH},
};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);
static OWNER_PID: AtomicU32 = AtomicU32::new(0);
static WRITER: OnceLock<Mutex<Writer>> = OnceLock::new();
const FORK_DISABLED: &str = "inherited Hunin Rust SDK state is disabled after fork until exec";
struct Writer {
    run: String,
    directory: String,
    producer: String,
    file: Option<File>,
    failure: Option<String>,
}
fn owns_state() -> bool {
    let pid = std::process::id();
    match OWNER_PID.load(Ordering::SeqCst) {
        owner if owner == pid => true,
        0 => OWNER_PID
            .compare_exchange(0, pid, Ordering::SeqCst, Ordering::SeqCst)
            .map_or_else(|owner| owner == pid, |_| true),
        _ => false,
    }
}
fn writer() -> Option<&'static Mutex<Writer>> {
    // Sequential owner publication precedes initializer/mutex access in every
    // parent thread. Reject inherited state before either primitive; exec
    // starts fresh state. This is not a general post-fork runtime guarantee.
    if !owns_state() {
        return None;
    }
    Some(WRITER.get_or_init(|| {
        Mutex::new(Writer {
            run: std::env::var("HUNIN_RUN_ID").unwrap_or_default(),
            directory: std::env::var("HUNIN_RECORD_DIR").unwrap_or_default(),
            producer: format!(
                "{}-{:x}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos()
            ),
            file: None,
            failure: None,
        })
    }))
}
fn epoch_us() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros()
        .min(9_007_199_254_740_991) as u64
}
fn id() -> Option<String> {
    let state = writer()?.lock().unwrap_or_else(|e| e.into_inner());
    Some(format!(
        "{}:{}",
        state.producer,
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ))
}
fn object(value: Value) -> Value {
    if value.is_object() {
        value
    } else {
        json!({})
    }
}
pub fn enabled() -> bool {
    let Some(writer) = writer() else {
        return false;
    };
    let state = writer.lock().unwrap_or_else(|e| e.into_inner());
    !state.run.is_empty() && !state.directory.is_empty() && state.failure.is_none()
}
pub fn last_error() -> Option<String> {
    let Some(writer) = writer() else {
        return Some(FORK_DISABLED.into());
    };
    writer
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .failure
        .clone()
}
fn emit(context: &Context, kind: &str, fields: Value) -> Option<String> {
    let record_id = id()?;
    let mut state = writer()?.lock().unwrap_or_else(|e| e.into_inner());
    if state.run.is_empty() || state.directory.is_empty() || state.failure.is_some() {
        return None;
    }
    let mut record = json!({"schema_version":1,"kind":kind,"id":record_id,"run_id":state.run,"producer_id":state.producer,"pid":std::process::id(),"tid":format!("{:?}",std::thread::current().id()),"timestamp_us":epoch_us(),"basis":"explicit"});
    if context.run_id == state.run {
        if let Some(value) = &context.context_id {
            record["context_id"] = json!(value);
        }
    }
    if let Some(fields) = fields.as_object() {
        for (key, value) in fields {
            record[key] = value.clone();
        }
    }
    let mut attributes = json!({"thread_id_source":"std::thread::ThreadId", "clock":"unix_epoch", "clock_alignment":"direct_epoch", "clock_source":"SystemTime epoch with Instant duration"});
    if let Some(items) = record["attributes"].as_object() {
        for (key, value) in items {
            attributes[key] = value.clone();
        }
    }
    record["attributes"] = attributes;
    let result = (|| -> std::io::Result<()> {
        if state.file.is_none() {
            fs::create_dir_all(&state.directory)?;
            let path = std::path::Path::new(&state.directory)
                .join(format!("sdk-rust-{}.jsonl", state.producer));
            let mut options = OpenOptions::new();
            options.create(true).append(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            state.file = Some(options.open(path)?);
        }
        let mut payload = serde_json::to_vec(&record)?;
        payload.push(b'\n');
        state.file.as_mut().unwrap().write_all(&payload)
    })();
    if let Err(error) = result {
        state.failure = Some(error.to_string());
        return None;
    }
    record["id"].as_str().map(str::to_owned)
}
pub fn flush() -> Result<(), String> {
    let mut state = writer()
        .ok_or_else(|| FORK_DISABLED.to_owned())?
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if let Some(file) = state.file.as_ref() {
        if let Err(error) = file.sync_all() {
            state.failure = Some(error.to_string());
        }
    };
    state.failure.clone().map_or(Ok(()), Err)
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Context {
    pub run_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
}
impl Context {
    pub fn current_run() -> Self {
        Self {
            run_id: writer()
                .map(|writer| writer.lock().unwrap_or_else(|e| e.into_inner()).run.clone())
                .unwrap_or_default(),
            ..Self::default()
        }
    }
    pub fn new(name: &str) -> Self {
        let mut context = Self::current_run();
        context.context_id = emit(&context, "context", json!({"name":name}));
        context
    }
    pub fn span(&self, name: &str) -> Span {
        Span::in_context(self, name)
    }
}
pub struct Span {
    pub id: Option<String>,
    context: Context,
    name: String,
    started: Instant,
    timestamp_us: u64,
    attributes: Map<String, Value>,
    ended: bool,
}
impl Span {
    pub fn new(name: &str) -> Self {
        Self::in_context(&Context::current_run(), name)
    }
    pub fn in_context(context: &Context, name: &str) -> Self {
        let active = enabled();
        let run = Context::current_run();
        Self {
            id: if active { id() } else { None },
            context: if context.run_id == run.run_id {
                context.clone()
            } else {
                run
            },
            name: name.into(),
            started: Instant::now(),
            timestamp_us: epoch_us(),
            attributes: Map::new(),
            ended: false,
        }
    }
    pub fn context(&self) -> Context {
        if !owns_state() {
            return Context::default();
        }
        Context {
            parent_id: self.id.clone(),
            ..self.context.clone()
        }
    }
    pub fn attribute(&mut self, key: impl Into<String>, value: impl Into<Value>) {
        self.attributes.insert(key.into(), value.into());
    }
    pub fn end(mut self) {
        self.finish();
    }
    fn finish(&mut self) {
        if self.ended || !owns_state() {
            return;
        }
        self.ended = true;
        if let Some(id) = &self.id {
            let mut fields = json!({"id":id,"name":self.name,"timestamp_us":self.timestamp_us,"duration_us":self.started.elapsed().as_micros().min(9_007_199_254_740_991)as u64,"attributes":self.attributes});
            if let Some(parent) = &self.context.parent_id {
                fields["parent_id"] = json!(parent);
            }
            emit(&self.context, "span", fields);
        }
    }
}
impl Drop for Span {
    fn drop(&mut self) {
        self.finish();
    }
}
pub fn counter(context: &Context, name: &str, value: f64, unit: &str) -> Option<String> {
    if !value.is_finite() {
        return None;
    }
    emit(
        context,
        "metric",
        json!({"name":name,"value":value,"unit":unit,"scope":if context.context_id.is_some(){"context"}else{"thread"},"attributes":{"temporality":"delta"}}),
    )
}
pub fn event(context: &Context, name: &str, attributes: Value) -> Option<String> {
    emit(
        context,
        "event",
        json!({"name":name,"attributes":object(attributes)}),
    )
}
pub fn relation(
    context: &Context,
    from: &str,
    to: &str,
    name: &str,
    evidence: &[String],
) -> Option<String> {
    let unique: std::collections::BTreeSet<_> = evidence.iter().collect();
    emit(
        context,
        "relation",
        json!({"from":from,"to":to,"relation":name,"evidence":unique}),
    )
}

#[cfg(test)]
mod fork_guard_tests {
    use super::*;
    use std::{sync::mpsc, time::Duration};

    fn foreign_process_probe() -> (mpsc::Receiver<bool>, std::thread::JoinHandle<()>) {
        let (sender, receiver) = mpsc::channel();
        let probe = std::thread::spawn(move || {
            let context = Context::new("foreign");
            let span = context.span("foreign-span");
            let rejected = !enabled()
                && context.run_id.is_empty()
                && context.context_id.is_none()
                && span.id.is_none()
                && span.context().run_id.is_empty()
                && event(&context, "foreign-event", json!({})).is_none()
                && flush().err().as_deref() == Some(FORK_DISABLED)
                && last_error().as_deref() == Some(FORK_DISABLED);
            drop(span);
            sender.send(rejected).unwrap();
        });
        (receiver, probe)
    }

    #[test]
    fn foreign_pid_is_rejected_before_in_progress_initializer_or_held_writer() {
        use std::{
            io::Read,
            process::{Command, Stdio},
            time::Instant,
        };
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--ignored",
                "--exact",
                "recording::fork_guard_tests::foreign_pid_probe",
                "--nocapture",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(6);
        let status = loop {
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if Instant::now() >= deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("isolated SDK synchronization probe exceeded its deadline");
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        let mut stderr = String::new();
        child
            .stderr
            .take()
            .unwrap()
            .read_to_string(&mut stderr)
            .unwrap();
        assert!(status.success(), "{stderr}");
    }

    #[test]
    #[ignore = "isolated process-global SDK synchronization control"]
    fn foreign_pid_probe() {
        let pid = std::process::id();
        OWNER_PID.store(pid, Ordering::SeqCst);
        let (started, initialized) = mpsc::channel();
        let (release, blocked) = mpsc::channel();
        let initializer = std::thread::spawn(move || {
            WRITER.get_or_init(|| {
                started.send(()).unwrap();
                blocked.recv().unwrap();
                Mutex::new(Writer {
                    run: String::new(),
                    directory: String::new(),
                    producer: "guard-control".into(),
                    file: None,
                    failure: None,
                })
            });
        });
        initialized.recv_timeout(Duration::from_secs(2)).unwrap();
        OWNER_PID.store(pid.wrapping_add(1).max(1), Ordering::SeqCst);
        let (result, probe) = foreign_process_probe();
        let initialization_result = result.recv_timeout(Duration::from_secs(2));
        OWNER_PID.store(pid, Ordering::SeqCst);
        release.send(()).unwrap();
        initializer.join().unwrap();
        probe.join().unwrap();
        assert_eq!(initialization_result, Ok(true));

        let held = writer().unwrap().lock().unwrap();
        OWNER_PID.store(pid.wrapping_add(1).max(1), Ordering::SeqCst);
        let (result, probe) = foreign_process_probe();
        let mutex_result = result.recv_timeout(Duration::from_secs(2));
        OWNER_PID.store(pid, Ordering::SeqCst);
        drop(held);
        probe.join().unwrap();
        assert_eq!(mutex_result, Ok(true));
    }
}
