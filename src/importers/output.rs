//! Fallible, operation-wide normalized output accounting, independent of RSS.
//! Bytes sum encoded records without transport delimiters. Frame visits count
//! attempted ancestry/copy work. Parsing and one Value assembly are synchronous;
//! input dictionaries and retained attachments have separate bounds.
use anyhow::{Result, bail};
use serde::Serialize;
use serde_json::Value;
use std::{fmt, io::Write, time::Instant};

#[derive(Clone, Copy)]
pub(super) struct Limits {
    pub rows: u64,
    pub bytes: u64,
    pub record_bytes: u64,
    pub frame_visits: u64,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            rows: 100_000,
            bytes: 128 * 1024 * 1024,
            record_bytes: 8 * 1024 * 1024,
            frame_visits: 2_000_000,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OutputDimension {
    Rows,
    Bytes,
    RecordBytes,
    FrameVisits,
}
impl OutputDimension {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rows => "normalized_rows",
            Self::Bytes => "normalized_bytes",
            Self::RecordBytes => "record_bytes",
            Self::FrameVisits => "frame_visits",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct NormalizationOutputLimit {
    pub dimension: OutputDimension,
    pub maximum: u64,
    pub observed: u64,
}
impl fmt::Display for NormalizationOutputLimit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "profile-normalization-output-v1 {} limit exceeded: maximum {}, observed {}",
            self.dimension.as_str(),
            self.maximum,
            self.observed
        )
    }
}
impl std::error::Error for NormalizationOutputLimit {}

#[derive(Default)]
pub(crate) struct OutputBudget {
    limits: Limits,
    rows: u64,
    bytes: u64,
    frames: u64,
    failed: Option<NormalizationOutputLimit>,
}
impl OutputBudget {
    #[cfg(test)]
    pub(super) fn with_limits(limits: Limits) -> Self {
        Self {
            limits,
            ..Default::default()
        }
    }
    pub(super) fn check(&self) -> Result<()> {
        if let Some(failure) = self.failed {
            return Err(failure.into());
        }
        Ok(())
    }
    fn fail(&mut self, dimension: OutputDimension, maximum: u64, observed: u64) -> anyhow::Error {
        let failure = NormalizationOutputLimit {
            dimension,
            maximum,
            observed,
        };
        self.failed = Some(failure);
        failure.into()
    }
}

pub(super) struct RecordBatch<'a> {
    records: Vec<Value>,
    budget: &'a mut OutputBudget,
    deadline: Option<Instant>,
}
pub(super) struct ReservedRecord(Value);

impl<'a> RecordBatch<'a> {
    pub fn new(budget: &'a mut OutputBudget, deadline: Option<Instant>) -> Self {
        Self {
            records: vec![],
            budget,
            deadline,
        }
    }
    pub fn check(&mut self) -> Result<()> {
        self.budget.check()?;
        if self
            .deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            bail!("profile-normalization-output-v1 checkpoint deadline elapsed");
        }
        Ok(())
    }
    pub fn len(&self) -> usize {
        self.records.len()
    }
    pub fn reserve(&mut self, record: Value) -> Result<ReservedRecord> {
        self.check()?;
        let rows = self.budget.rows + 1;
        if rows > self.budget.limits.rows {
            return Err(self
                .budget
                .fail(OutputDimension::Rows, self.budget.limits.rows, rows));
        }
        let size = self.fragment(&record)?;
        self.budget.rows = rows;
        self.budget.bytes += size;
        Ok(ReservedRecord(record))
    }
    pub fn append_reserved(&mut self, record: ReservedRecord) -> Result<()> {
        self.check()?;
        self.records.push(record.0);
        Ok(())
    }
    pub fn push(&mut self, record: Value) -> Result<()> {
        let reserved = self.reserve(record)?;
        self.append_reserved(reserved)
    }
    pub fn extend(&mut self, records: impl IntoIterator<Item = Value>) -> Result<()> {
        for record in records {
            self.push(record)?;
        }
        Ok(())
    }
    pub fn finish(mut self) -> Result<Vec<Value>> {
        self.check()?;
        Ok(self.records)
    }
    pub fn frame(&mut self) -> Result<()> {
        self.check()?;
        self.budget.frames += 1;
        if self.budget.frames > self.budget.limits.frame_visits {
            return Err(self.budget.fail(
                OutputDimension::FrameVisits,
                self.budget.limits.frame_visits,
                self.budget.frames,
            ));
        }
        Ok(())
    }
    pub fn fragment(&mut self, value: &impl Serialize) -> Result<u64> {
        self.fragment_after(value, 0)
    }
    pub fn fragment_after(&mut self, value: &impl Serialize, prefix: u64) -> Result<u64> {
        self.check()?;
        let remaining = self.budget.limits.bytes.saturating_sub(self.budget.bytes);
        let maximum = self.budget.limits.record_bytes.min(remaining);
        let mut counter = Counter {
            bytes: 0,
            maximum: maximum.saturating_sub(prefix),
            exceeded: prefix > maximum,
        };
        let result = serde_json::to_writer(&mut counter, value);
        if counter.exceeded {
            let observed = maximum.saturating_add(1);
            return Err(if self.budget.limits.record_bytes <= remaining {
                self.budget.fail(
                    OutputDimension::RecordBytes,
                    self.budget.limits.record_bytes,
                    observed,
                )
            } else {
                self.budget.fail(
                    OutputDimension::Bytes,
                    self.budget.limits.bytes,
                    self.budget.bytes.saturating_add(observed),
                )
            });
        }
        result?;
        self.check()?;
        Ok(counter.bytes)
    }
    pub fn checked_clone(&mut self, value: &Value) -> Result<Value> {
        self.fragment(value)?;
        Ok(value.clone())
    }
    pub fn known_fragment_bytes(&mut self, bytes: u64) -> Result<()> {
        self.check()?;
        let remaining = self.budget.limits.bytes.saturating_sub(self.budget.bytes);
        let maximum = self.budget.limits.record_bytes.min(remaining);
        if bytes > maximum {
            return Err(if self.budget.limits.record_bytes <= remaining {
                self.budget.fail(
                    OutputDimension::RecordBytes,
                    self.budget.limits.record_bytes,
                    maximum.saturating_add(1),
                )
            } else {
                self.budget.fail(
                    OutputDimension::Bytes,
                    self.budget.limits.bytes,
                    self.budget.bytes.saturating_add(maximum).saturating_add(1),
                )
            });
        }
        Ok(())
    }
    pub fn clone_stack(&mut self, frames: &[Value]) -> Result<Vec<Value>> {
        Ok(StackBuilder::from_slice(frames, self)?.finish())
    }
}

struct Counter {
    bytes: u64,
    maximum: u64,
    exceeded: bool,
}
impl Write for Counter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.bytes = self.bytes.saturating_add(bytes.len() as u64);
        if self.bytes > self.maximum {
            self.exceeded = true;
            return Err(std::io::Error::other("normalized output counter exceeded"));
        }
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

pub(super) struct StackBuilder {
    frames: Vec<Value>,
    bytes: u64,
}
impl StackBuilder {
    pub fn new() -> Self {
        Self {
            frames: vec![],
            bytes: 2,
        }
    }
    pub fn from_slice(frames: &[Value], batch: &mut RecordBatch<'_>) -> Result<Self> {
        let mut stack = Self::new();
        for frame in frames {
            stack.push_borrowed(frame, batch)?;
        }
        Ok(stack)
    }
    pub fn push_borrowed(&mut self, frame: &Value, batch: &mut RecordBatch<'_>) -> Result<()> {
        batch.frame()?;
        let prefix = self.bytes + u64::from(!self.frames.is_empty());
        let bytes = batch.fragment_after(frame, prefix)?;
        self.frames.push(frame.clone());
        self.bytes = prefix + bytes;
        Ok(())
    }
    pub fn push_owned(&mut self, frame: Value, batch: &mut RecordBatch<'_>) -> Result<()> {
        batch.frame()?;
        let prefix = self.bytes + u64::from(!self.frames.is_empty());
        let bytes = batch.fragment_after(&frame, prefix)?;
        self.frames.push(frame);
        self.bytes = prefix + bytes;
        Ok(())
    }
    pub fn finish(self) -> Vec<Value> {
        self.frames
    }
    pub fn as_slice(&self) -> &[Value] {
        &self.frames
    }
    pub fn append(&mut self, mut other: Self, batch: &mut RecordBatch<'_>) -> Result<()> {
        let bytes = self.bytes + other.bytes - 2
            + u64::from(!self.frames.is_empty() && !other.frames.is_empty());
        batch.known_fragment_bytes(bytes)?;
        self.frames.append(&mut other.frames);
        self.bytes = bytes;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
