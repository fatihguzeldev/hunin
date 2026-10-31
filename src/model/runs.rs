use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

/// Recording state is distinct from target correctness and capture completeness.
/// Unknown historical labels remain readable without becoming known outcomes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunStatus {
    Running,
    Completed,
    Failed,
    Canceled,
    Unknown(String),
}

impl RunStatus {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Canceled => "canceled",
            Self::Unknown(value) => value,
        }
    }
}

impl From<String> for RunStatus {
    fn from(value: String) -> Self {
        match value.as_str() {
            "running" => Self::Running,
            "completed" => Self::Completed,
            "failed" => Self::Failed,
            "canceled" => Self::Canceled,
            _ => Self::Unknown(value),
        }
    }
}

impl From<&str> for RunStatus {
    fn from(value: &str) -> Self {
        Self::from(value.to_owned())
    }
}

impl PartialEq<&str> for RunStatus {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

impl Serialize for RunStatus {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for RunStatus {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer).map(Self::from)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Run {
    pub id: String,
    pub name: String,
    pub status: RunStatus,
    pub command: Vec<String>,
    pub cwd: String,
    pub runtime: String,
    pub started_at_us: u64,
    pub finished_at_us: Option<u64>,
    pub exit_code: Option<i32>,
    pub diagnostics: Vec<String>,
    pub source: Value,
    pub correctness: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Artifact {
    pub id: String,
    pub name: String,
    pub size: u64,
    pub sha256: String,
    pub format: String,
}

impl Artifact {
    /// Run-owned reference identity; content identity remains the separate SHA-256.
    pub fn reference_id(sha256: &str, name: &str, format: &str) -> String {
        use sha2::{Digest, Sha256};
        let identity = serde_json::to_vec(&(sha256, name, format))
            .expect("serializing an artifact identity tuple of strings cannot fail");
        format!("{:x}", Sha256::digest(identity))
    }
}
