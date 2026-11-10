use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::records::{IngestOutcome, Record, SourceHandle};

pub const PROTOCOL_VERSION: u32 = 1;
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;
const PROJECT_INGRESS_BYTES: usize = 64 * 1024 * 1024;
const SOURCE_CONNECTIONS: usize = 8;

#[derive(Debug, thiserror::Error)]
pub enum ChannelError {
    #[error("producer channel io: {0}")]
    Io(#[from] io::Error),
    #[error("invalid producer json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("producer record rejected: {0}")]
    Store(#[from] crate::project::Error),
    #[error("invalid producer protocol: {0}")]
    Invalid(&'static str),
    #[error("producer channel deadline exceeded")]
    Deadline,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProducerMessage {
    Hello {
        version: u32,
        project: String,
        source: String,
        credential: String,
        required_semantics: Vec<String>,
    },
    Ready {
        coverage: Value,
    },
    Batch {
        delivery: String,
        records: Vec<Record>,
    },
    Gap {
        id: String,
        detail: Value,
    },
    Finish {
        outcome: Value,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Acknowledgment {
    pub kind: String,
    pub durable: bool,
    pub revision: Option<u64>,
    pub delivery: Option<String>,
    pub publication: Option<IngestOutcome>,
}

#[derive(Clone, Debug)]
pub enum ProducerEvent {
    Ready(Value),
    Published(IngestOutcome),
    Gap(Value),
    Finished(Value),
}

struct Budget {
    used: AtomicUsize,
    sources: Mutex<HashMap<(String, String), Weak<AtomicUsize>>>,
}

struct Reservation {
    budget: Arc<Budget>,
    bytes: usize,
}

impl Drop for Reservation {
    fn drop(&mut self) {
        self.budget.used.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}

fn project_budget(source: &SourceHandle) -> Arc<Budget> {
    static BUDGETS: OnceLock<Mutex<HashMap<std::path::PathBuf, Weak<Budget>>>> = OnceLock::new();
    let mut budgets = BUDGETS
        .get_or_init(Mutex::default)
        .lock()
        .expect("ingress budgets");
    budgets.retain(|_, budget| budget.strong_count() > 0);
    let key = source.project().directory().to_path_buf();
    if let Some(budget) = budgets.get(&key).and_then(Weak::upgrade) {
        return budget;
    }
    let budget = Arc::new(Budget {
        used: AtomicUsize::new(0),
        sources: Mutex::new(HashMap::new()),
    });
    budgets.insert(key, Arc::downgrade(&budget));
    budget
}

impl Budget {
    fn source_connections(&self, source: &SourceHandle) -> Arc<AtomicUsize> {
        let mut sources = self.sources.lock().expect("source ingress limits");
        sources.retain(|_, count| count.strong_count() > 0);
        let key = (source.run().id, source.id().to_owned());
        if let Some(count) = sources.get(&key).and_then(Weak::upgrade) {
            return count;
        }
        let count = Arc::new(AtomicUsize::new(0));
        sources.insert(key, Arc::downgrade(&count));
        count
    }

    fn reserve(self: &Arc<Self>, bytes: usize) -> Result<Reservation, ChannelError> {
        self.used
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                used.checked_add(bytes)
                    .filter(|n| *n <= PROJECT_INGRESS_BYTES)
            })
            .map_err(|_| ChannelError::Invalid("project ingress budget exhausted"))?;
        Ok(Reservation {
            budget: self.clone(),
            bytes,
        })
    }
}

fn remaining(until: Instant) -> Result<Duration, ChannelError> {
    let left = until.saturating_duration_since(Instant::now());
    if left.is_zero() {
        Err(ChannelError::Deadline)
    } else {
        Ok(left)
    }
}

fn read_exact(
    stream: &mut TcpStream,
    bytes: &mut [u8],
    until: Instant,
) -> Result<(), ChannelError> {
    let mut position = 0;
    while position < bytes.len() {
        stream.set_read_timeout(Some(remaining(until)?))?;
        match stream.read(&mut bytes[position..]) {
            Ok(0) => {
                return Err(
                    io::Error::new(io::ErrorKind::UnexpectedEof, "partial producer frame").into(),
                );
            }
            Ok(n) => position += n,
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                return Err(ChannelError::Deadline);
            }
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

fn read_message(
    stream: &mut TcpStream,
    until: Instant,
    budget: &Arc<Budget>,
) -> Result<(ProducerMessage, Reservation), ChannelError> {
    let mut prefix = [0; 4];
    read_exact(stream, &mut prefix, until)?;
    let size = u32::from_be_bytes(prefix) as usize;
    if size == 0 || size > MAX_FRAME_BYTES {
        return Err(ChannelError::Invalid(
            "frame length must be 1..=1048576 bytes",
        ));
    }
    let reservation = budget.reserve(size)?;
    let mut bytes = vec![0; size];
    read_exact(stream, &mut bytes, until)?;
    Ok((serde_json::from_slice(&bytes)?, reservation))
}

fn write_json<T: Serialize>(
    stream: &mut TcpStream,
    value: &T,
    until: Instant,
) -> Result<(), ChannelError> {
    let payload = serde_json::to_vec(value)?;
    if payload.is_empty() || payload.len() > MAX_FRAME_BYTES {
        return Err(ChannelError::Invalid("outgoing frame exceeds limit"));
    }
    let prefix = (payload.len() as u32).to_be_bytes();
    for mut bytes in [prefix.as_slice(), payload.as_slice()] {
        while !bytes.is_empty() {
            stream.set_write_timeout(Some(remaining(until)?))?;
            let written = stream.write(bytes)?;
            if written == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "producer frame write stopped",
                )
                .into());
            }
            bytes = &bytes[written..];
        }
    }
    Ok(())
}

pub struct ProducerListener {
    listener: TcpListener,
    source: SourceHandle,
    credential: String,
    budget: Arc<Budget>,
    connections: Arc<AtomicUsize>,
}

impl ProducerListener {
    pub fn bind(source: SourceHandle) -> Result<Self, ChannelError> {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
        listener.set_nonblocking(true)?;
        let mut bytes = [0; 32];
        getrandom::fill(&mut bytes)
            .map_err(|_| ChannelError::Invalid("session credential generation failed"))?;
        let credential = bytes.iter().map(|b| format!("{b:02x}")).collect();
        let budget = project_budget(&source);
        let connections = budget.source_connections(&source);
        Ok(Self {
            listener,
            source,
            credential,
            budget,
            connections,
        })
    }

    pub fn address(&self) -> Result<SocketAddr, ChannelError> {
        Ok(self.listener.local_addr()?)
    }
    pub fn credential(&self) -> &str {
        &self.credential
    }

    pub fn accept(&self, timeout: Duration) -> Result<ProducerSession, ChannelError> {
        let until = Instant::now()
            .checked_add(timeout)
            .ok_or(ChannelError::Invalid("invalid session deadline"))?;
        let (stream, peer) = loop {
            match self.listener.accept() {
                Ok(value) => break value,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(2).min(remaining(until)?));
                }
                Err(e) => return Err(e.into()),
            }
        };
        if !peer.ip().is_loopback() {
            return Err(ChannelError::Invalid("non-loopback producer"));
        }
        stream.set_nonblocking(false)?;
        self.connections
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| {
                (n < SOURCE_CONNECTIONS).then_some(n + 1)
            })
            .map_err(|_| ChannelError::Invalid("source connection limit reached"))?;
        let mut session = ProducerSession {
            stream,
            source: self.source.clone(),
            budget: self.budget.clone(),
            connections: self.connections.clone(),
            ready: false,
            terminal: false,
        };
        let (message, _reservation) = read_message(&mut session.stream, until, &self.budget)?;
        let ProducerMessage::Hello {
            version,
            project,
            source,
            credential,
            required_semantics,
        } = message
        else {
            return Err(ChannelError::Invalid("first message must be hello"));
        };
        let token_equal = credential.len() == self.credential.len()
            && credential
                .bytes()
                .zip(self.credential.bytes())
                .fold(0u8, |difference, (a, b)| difference | (a ^ b))
                == 0;
        if version != PROTOCOL_VERSION
            || project != self.source.project().id()
            || source != self.source.id()
            || !token_equal
        {
            return Err(ChannelError::Invalid(
                "incompatible version or foreign source/session binding",
            ));
        }
        if required_semantics.len() > 16
            || required_semantics
                .iter()
                .any(|s| s != "structured-records-v1")
        {
            return Err(ChannelError::Invalid("unsupported required semantics"));
        }
        session.ack("session", None, None, None, until)?;
        Ok(session)
    }
}

pub struct ProducerSession {
    stream: TcpStream,
    source: SourceHandle,
    budget: Arc<Budget>,
    connections: Arc<AtomicUsize>,
    ready: bool,
    terminal: bool,
}

impl ProducerSession {
    fn ack(
        &mut self,
        kind: &str,
        revision: Option<u64>,
        delivery: Option<String>,
        publication: Option<IngestOutcome>,
        until: Instant,
    ) -> Result<(), ChannelError> {
        write_json(
            &mut self.stream,
            &Acknowledgment {
                kind: kind.into(),
                durable: revision.is_some(),
                revision,
                delivery,
                publication,
            },
            until,
        )
    }

    pub fn next(&mut self, timeout: Duration) -> Result<ProducerEvent, ChannelError> {
        if self.terminal {
            return Err(ChannelError::Invalid("producer session is closed"));
        }
        let until = Instant::now()
            .checked_add(timeout)
            .ok_or(ChannelError::Invalid("invalid message deadline"))?;
        let (message, _reservation) = read_message(&mut self.stream, until, &self.budget)?;
        match message {
            ProducerMessage::Hello { .. } => Err(ChannelError::Invalid(
                "hello is only valid at session start",
            )),
            ProducerMessage::Ready { coverage } => {
                let revision = self.source.ready(coverage.clone())?;
                self.ready = true;
                self.ack("ready", Some(revision), None, None, until)?;
                Ok(ProducerEvent::Ready(coverage))
            }
            ProducerMessage::Batch { delivery, records } => {
                if !self.ready {
                    return Err(ChannelError::Invalid(
                        "producer must declare readiness before data",
                    ));
                }
                let outcome = self.source.append_batch(&delivery, &records)?;
                self.ack(
                    "publication",
                    Some(outcome.revision),
                    Some(delivery),
                    Some(outcome.clone()),
                    until,
                )?;
                Ok(ProducerEvent::Published(outcome))
            }
            ProducerMessage::Gap { id, detail } => {
                let revision = self.source.report_gap(&id, detail.clone())?;
                self.ack("gap", Some(revision), None, None, until)?;
                Ok(ProducerEvent::Gap(detail))
            }
            ProducerMessage::Finish { outcome } => {
                let revision = self.source.close(outcome.clone())?;
                self.terminal = true;
                self.ack("closed", Some(revision), None, None, until)?;
                Ok(ProducerEvent::Finished(outcome))
            }
        }
    }
}

impl Drop for ProducerSession {
    fn drop(&mut self) {
        self.connections.fetch_sub(1, Ordering::AcqRel);
    }
}

pub fn send(
    stream: &mut TcpStream,
    message: &ProducerMessage,
    timeout: Duration,
) -> Result<Acknowledgment, ChannelError> {
    let until = Instant::now()
        .checked_add(timeout)
        .ok_or(ChannelError::Invalid("invalid message deadline"))?;
    write_json(stream, message, until)?;
    let mut prefix = [0; 4];
    read_exact(stream, &mut prefix, until)?;
    let size = u32::from_be_bytes(prefix) as usize;
    if size == 0 || size > MAX_FRAME_BYTES {
        return Err(ChannelError::Invalid("invalid acknowledgment length"));
    }
    let mut bytes = vec![0; size];
    read_exact(stream, &mut bytes, until)?;
    Ok(serde_json::from_slice(&bytes)?)
}
