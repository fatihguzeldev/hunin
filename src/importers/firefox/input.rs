//! Interpret the related native inputs as one format-owned evidence context.
use super::{images::Images, lifetimes::Timeline, profile, sampling};
use crate::importers::{BinarySnapshot, OutputBudget, output::RecordBatch};
use anyhow::{Context, Result};
use serde_json::Value;
use std::path::Path;

pub(in crate::importers) struct Input {
    raw: Value,
    lifetimes: Option<Timeline>,
    images: Images,
}

impl Input {
    pub(in crate::importers) fn load(
        path: &Path,
        raw: &[u8],
        raw_sha256: &str,
        lifetime_sha256: Option<&str>,
        native_context: Option<&Path>,
        budget: &mut OutputBudget,
    ) -> Result<Self> {
        let mut raw: Value = serde_json::from_slice(raw)?;
        let lifetimes = Timeline::load(
            &path.with_extension("lifetimes.json"),
            raw_sha256,
            lifetime_sha256,
            &raw,
            budget,
        )?;
        if let Some(timeline) = &lifetimes {
            let libraries = raw["libs"]
                .as_array_mut()
                .context("native library dictionary missing")?;
            for library in timeline.libraries() {
                if !libraries.contains(library) {
                    libraries.push(library.clone());
                }
            }
        }
        let images = match native_context {
            Some(path) => Images::from_context(&raw, path, raw_sha256)?,
            None => Images::inspect(&raw),
        };
        Ok(Self {
            raw,
            lifetimes,
            images,
        })
    }

    pub(in crate::importers) fn image_fingerprint(&self) -> Value {
        self.images.fingerprint()
    }

    pub(in crate::importers) fn decode_into(
        &self,
        path: &Path,
        run_id: &str,
        key: &str,
        records: &mut RecordBatch<'_>,
    ) -> Result<Vec<String>> {
        let mut diagnostics = profile::decode_into(
            path,
            &self.raw,
            run_id,
            key,
            &self.images,
            self.lifetimes.as_ref(),
            records,
        )?;
        if self.lifetimes.is_none()
            && self.raw["meta"]["oscpu"]
                .as_str()
                .is_some_and(|os| os.to_ascii_lowercase().starts_with("windows"))
        {
            diagnostics.push(sampling::LEGACY_DIAGNOSTIC.into());
        }
        Ok(diagnostics)
    }

    pub(in crate::importers) fn into_evidence(
        self,
        raw_sha256: &str,
    ) -> (Value, Vec<BinarySnapshot>) {
        (
            self.images.context(raw_sha256),
            self.images.into_snapshots(),
        )
    }
}
