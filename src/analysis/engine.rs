//! Analyzer engine state, admission configuration and package registration.
use super::package;
use crate::storage::Store;
use anyhow::Result;
use serde_json::Value;
use std::{
    collections::HashMap,
    sync::{Arc, atomic::AtomicBool},
};
use tokio::sync::{Mutex, Semaphore, watch};

#[derive(Clone)]
pub struct Engine {
    pub(super) store: Store,
    pub(super) closing: Arc<AtomicBool>,
    pub(super) owner: crate::host::process::owner::Owner,
    pub(super) active: Arc<Mutex<HashMap<String, watch::Sender<bool>>>>,
    pub(super) slots: Arc<Semaphore>,
    pub(super) maximum_in_flight: usize,
}
impl Engine {
    pub fn new(store: Store) -> Self {
        Self::with_admission_limit(store, 2).expect("the default admission limit is valid")
    }
    /// Bound attempts shared by this Engine and its clones, from preparation
    /// through terminal retention/publication. Separate Engines are independent;
    /// this is neither a memory quota nor a lock across processes or a store.
    pub fn with_admission_limit(store: Store, maximum_in_flight: usize) -> Result<Self> {
        anyhow::ensure!(
            maximum_in_flight > 0 && maximum_in_flight <= Semaphore::MAX_PERMITS,
            "analysis admission limit must be positive and supported by the semaphore"
        );
        Ok(Self {
            store,
            closing: Arc::new(AtomicBool::new(false)),
            owner: crate::host::process::owner::Owner::current(),
            active: Arc::new(Mutex::new(HashMap::new())),
            slots: Arc::new(Semaphore::new(maximum_in_flight)),
            maximum_in_flight,
        })
    }
    pub fn register(&self, params: &Value) -> Result<Value> {
        package::register(&self.store, params)
    }
}
