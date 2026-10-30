//! Persistence boundary; each module owns its SQL and artifact lifecycle.
use std::path::{Path, PathBuf};

mod artifacts;
pub(crate) use artifacts::ArtifactByteLimit;
mod bootstrap;
mod count_requests;
mod documents;
pub use bootstrap::SCHEMA_VERSION;
mod inspection;
mod maintenance;
pub(crate) use maintenance::ActiveAnalysisRemoval;
mod metadata;
pub use metadata::{MetadataPageBudgetExceeded, MetadataPageDimension};
mod observations;
mod publication;
mod runs;
pub use inspection::{
    InspectionArea, InspectionFinding, InspectionLimit, InspectionLimits, InspectionPathKind,
    InspectionScan, InspectionScanStatus, StorageInspection, WorkspaceMetadataReference,
};
pub(crate) use observations::RecordReadLimit;
pub use observations::RecordsSnapshot;
pub use observations::{
    QueryReadPort, QuerySnapshot, ReadBudgetExceeded, ReadDimension, ReadLimits, ReadUsage,
};

#[derive(Clone)]
pub struct Store {
    root: PathBuf,
}

impl Store {
    pub fn root(&self) -> &Path {
        &self.root
    }
}
