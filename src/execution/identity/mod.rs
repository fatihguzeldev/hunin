//! Pre-launch source, environment and explicitly declared input identity.
mod environment;
mod inputs;
mod source;

pub(super) use environment::environment;
pub(super) use inputs::workload_inputs;
pub(super) use source::source;

use sha2::{Digest, Sha256};
use std::{fs, io::Read, path::Path};

fn file_hash(path: &Path) -> std::io::Result<String> {
    let mut file = fs::File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(format!("{:x}", digest.finalize()))
}
