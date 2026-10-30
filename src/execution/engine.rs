//! Recording engine state, admission configuration and startup recovery.
use super::recovery;
use crate::{host::process::owner::Owner, storage::Store};
use anyhow::Result;
use std::{
    collections::HashMap,
    sync::{Arc, atomic::AtomicBool},
};
use tokio::sync::{Mutex, Semaphore, watch};

#[derive(Clone)]
pub struct Engine {
    pub(super) store: Store,
    pub(super) active: Arc<Mutex<HashMap<String, watch::Sender<bool>>>>,
    pub(super) closing: Arc<AtomicBool>,
    pub(super) owner: Owner,
    pub(super) slots: Arc<Semaphore>,
    pub(super) maximum_in_flight: usize,
}

impl Engine {
    pub fn new(store: Store) -> Self {
        Self::with_admission_limit(store, 4)
            .expect("the default recording admission limit is valid")
    }
    /// Bound preparation, execution and terminal retention for this Engine and
    /// its clones. Separate Engines are independent; this is not a memory or
    /// cross-process/store quota.
    pub fn with_admission_limit(store: Store, maximum_in_flight: usize) -> Result<Self> {
        Self::initialize(store, maximum_in_flight, false)
    }

    pub(crate) fn try_new(store: Store) -> Result<Self> {
        Self::initialize(store, 4, true)
    }

    fn initialize(store: Store, maximum_in_flight: usize, strict_recovery: bool) -> Result<Self> {
        anyhow::ensure!(
            maximum_in_flight > 0 && maximum_in_flight <= Semaphore::MAX_PERMITS,
            "recording admission limit must be positive and supported by the semaphore"
        );
        if let Err(error) = recovery::recover_abandoned(&store) {
            if strict_recovery {
                return Err(error.context("recording owner recovery failed"));
            }
            eprintln!("recording owner recovery failed: {error:#}");
        }
        Ok(Self {
            store,
            active: Arc::new(Mutex::new(HashMap::new())),
            closing: Arc::new(AtomicBool::new(false)),
            owner: Owner::current(),
            slots: Arc::new(Semaphore::new(maximum_in_flight)),
            maximum_in_flight,
        })
    }
    pub fn store(&self) -> &Store {
        &self.store
    }
}
