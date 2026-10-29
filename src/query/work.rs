use crate::storage::ReadLimits;
use anyhow::Result;
use std::fmt;

#[derive(Clone, Debug)]
/// Cooperative work limits for a selected recording query. These cap serialized
/// input and counted operations, not allocator usage or elapsed time. One JSON
/// decode, filesystem/SQLite operation, or bounded sort cannot be preempted.
pub struct QueryLimits {
    pub read: ReadLimits,
    pub maximum_frame_visits: u64,
    pub maximum_aggregate_entries: u64,
}
impl Default for QueryLimits {
    fn default() -> Self {
        Self {
            read: ReadLimits::default(),
            maximum_frame_visits: 2_000_000,
            maximum_aggregate_entries: 100_000,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkDimension {
    FrameVisits,
    AggregateEntries,
}
impl WorkDimension {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::FrameVisits => "frame_visits",
            Self::AggregateEntries => "aggregate_entries",
        }
    }
}
#[derive(Debug)]
pub struct WorkBudgetExceeded {
    pub dimension: WorkDimension,
    pub limit: u64,
    pub observed: u64,
}
impl fmt::Display for WorkBudgetExceeded {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "query {} budget exceeded: observed {}, limit {}",
            self.dimension.as_str(),
            self.observed,
            self.limit
        )
    }
}
impl std::error::Error for WorkBudgetExceeded {}

pub(crate) struct WorkBudget {
    limits: QueryLimits,
    frames: u64,
    entries: u64,
    checkpoint: Box<dyn FnMut() -> Result<()> + Send>,
}
impl WorkBudget {
    pub(crate) fn new(
        limits: QueryLimits,
        checkpoint: impl FnMut() -> Result<()> + Send + 'static,
    ) -> Self {
        Self {
            limits,
            frames: 0,
            entries: 0,
            checkpoint: Box::new(checkpoint),
        }
    }
    pub(crate) fn unlimited() -> Self {
        Self::new(
            QueryLimits {
                maximum_frame_visits: u64::MAX,
                maximum_aggregate_entries: u64::MAX,
                ..Default::default()
            },
            || Ok(()),
        )
    }
    pub(crate) fn check(&mut self) -> Result<()> {
        (self.checkpoint)()
    }
    pub(crate) fn frame(&mut self) -> Result<()> {
        self.check()?;
        self.frames = self.frames.saturating_add(1);
        if self.frames > self.limits.maximum_frame_visits {
            return Err(WorkBudgetExceeded {
                dimension: WorkDimension::FrameVisits,
                limit: self.limits.maximum_frame_visits,
                observed: self.frames,
            }
            .into());
        }
        Ok(())
    }
    pub(crate) fn entry(&mut self) -> Result<()> {
        self.check()?;
        self.entries = self.entries.saturating_add(1);
        if self.entries > self.limits.maximum_aggregate_entries {
            return Err(WorkBudgetExceeded {
                dimension: WorkDimension::AggregateEntries,
                limit: self.limits.maximum_aggregate_entries,
                observed: self.entries,
            }
            .into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[derive(Debug)]
    struct Abandoned;
    impl fmt::Display for Abandoned {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("reader abandoned")
        }
    }
    impl std::error::Error for Abandoned {}

    #[test]
    fn cancellation_stops_within_one_stack_and_preserves_its_typed_origin() {
        let mut checkpoints = 0;
        let mut work = WorkBudget::new(QueryLimits::default(), move || {
            checkpoints += 1;
            if checkpoints == 8 {
                return Err(Abandoned.into());
            }
            Ok(())
        });
        let record = json!({"kind":"sample","weight":1,"weight_unit":"samples","stack":vec![json!({"function":"recursive"});1000]});
        let mut page = crate::query::profiles::HotspotAccumulator::profile("samples");
        let error = page.add(&record, &mut work).unwrap_err();
        assert!(error.downcast_ref::<Abandoned>().is_some());
        assert!(work.frames < 1000);
    }
}
