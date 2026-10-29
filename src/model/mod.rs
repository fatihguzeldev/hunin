mod admission;
pub use admission::CapacityExceeded;
pub(crate) mod identity;
pub(crate) mod observations;
pub(crate) mod relations;
pub mod runs;

pub fn now_us() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros() as u64
}
