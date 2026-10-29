//! Blocking metadata and bounded content reads; caller owns admission/interest.
use super::{metadata_page, workers::ReadInterest};
use crate::storage::Store;
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::{
    fs,
    io::{Read, Seek, SeekFrom},
};

pub(super) fn query(
    store: &Store,
    name: &str,
    params: &Value,
    limit: usize,
    cursor: usize,
    interest: &ReadInterest,
) -> Result<Value> {
    let id = || params["run_id"].as_str().context("run_id required");
    interest.check()?;
    let result = match name {
        "runs.list" => {
            let mut runs = store.runs(limit + 1, cursor)?;
            let more = runs.len() > limit;
            runs.truncate(limit);
            json!({"runs":runs,"next_cursor":more.then(||(cursor+limit).to_string())})
        }
        "runs.metadata" => {
            let run_id = id()?;
            let run = store.run(run_id)?;
            let interest = interest.clone();
            let identity = store.with_query_snapshot(
                run_id,
                Default::default(),
                move || interest.check(),
                |_| Ok(()),
            );
            match identity {
                Ok(snapshot) => json!({"run":run,"result_identity":snapshot.result_identity}),
                Err(error) if error.is::<crate::storage::ReadBudgetExceeded>() => {
                    json!({"run":run,"result_identity_diagnostic":format!("Selection identity unavailable within the metadata read budget: {error:#}")})
                }
                Err(error) => return Err(error),
            }
        }
        "contexts.query" => {
            let interest = interest.clone();
            let mut selected = store
                .with_query_snapshot(
                    id()?,
                    Default::default(),
                    move || interest.check(),
                    |port| {
                        let mut rows = Vec::new();
                        port.visit_page(&["context", "relation"], limit + 1, cursor, |text| {
                            rows.push(serde_json::from_str::<Value>(text)?);
                            Ok(())
                        })?;
                        Ok(rows)
                    },
                )?
                .value;
            let more = selected.len() > limit;
            selected.truncate(limit);
            json!({"contexts":selected.iter().filter(|r|r["kind"]=="context").collect::<Vec<_>>(),"relations":selected.iter().filter(|r|r["kind"]=="relation").collect::<Vec<_>>(),"next_cursor":more.then(||(cursor+limit).to_string())})
        }
        "evidence.get" => {
            let wanted = serde_json::from_value::<Vec<String>>(params["ids"].clone())?;
            let interest = interest.clone();
            let rows = store
                .with_query_snapshot(
                    id()?,
                    Default::default(),
                    move || interest.check(),
                    |port| {
                        let mut rows = Vec::new();
                        port.visit_ids(&wanted, |text| {
                            rows.push(serde_json::from_str::<Value>(text)?);
                            Ok(())
                        })?;
                        Ok(rows)
                    },
                )?
                .value;
            json!({"records":rows})
        }
        "artifacts.list" => metadata_page(
            "artifacts",
            store.artifacts_page(id()?, limit + 1, cursor)?,
            limit,
            cursor,
        ),
        "artifacts.read" => {
            let path = store.artifact_path(id()?, params["artifact_id"].as_str().unwrap())?;
            let mut file = fs::File::open(path)?;
            let size = file.metadata()?.len();
            let start = params["offset"].as_u64().unwrap_or(0);
            if start > size {
                bail!("artifact offset outside content");
            }
            file.seek(SeekFrom::Start(start))?;
            interest.check()?;
            let mut bytes = Vec::new();
            file.take(limit.min(5000) as u64).read_to_end(&mut bytes)?;
            let end = start + bytes.len() as u64;
            json!({"content":String::from_utf8_lossy(&bytes),"encoding":"utf8_lossy","next_offset":(end<size).then_some(end),"size":size})
        }
        "source.read" => crate::source::read(store, params)?,
        "scenarios.list" => metadata_page(
            "scenarios",
            store.documents_page("scenarios", None, limit + 1, cursor)?,
            limit,
            cursor,
        ),
        "analyses.list" => metadata_page(
            "analyses",
            store.documents_page("analyses", None, limit + 1, cursor)?,
            limit,
            cursor,
        ),
        _ => bail!("unknown metadata read"),
    };
    interest.check()?;
    Ok(result)
}
