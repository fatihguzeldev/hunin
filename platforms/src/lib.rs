pub mod native;
pub mod process;
pub mod recording;
pub mod tools;

mod os;

pub use process::{
    ProcessInstance, ResourceSnapshot, discover_processes, inspect_process, read_resources,
};
