pub mod process;
pub mod recording;

mod os;

pub use process::{
    ProcessInstance, ResourceSnapshot, discover_processes, inspect_process, read_resources,
};
