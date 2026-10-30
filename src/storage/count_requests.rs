//! Atomic retry reservations for additional counting recordings.
use super::{Store, documents::write_document};
use anyhow::Result;
use rusqlite::{OptionalExtension, TransactionBehavior, params};
use serde_json::{Value, json};

impl Store {
    pub(crate) fn claim_count_request(
        &self,
        key: &str,
        fingerprint: &str,
        parent: &str,
    ) -> Result<(bool, Value)> {
        let mut connection = self.connection()?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing: Option<String> = tx
            .query_row(
                "SELECT payload FROM documents WHERE namespace='call-count-requests' AND id=?1",
                [key],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(text) = existing {
            let mut value: Value = serde_json::from_str(&text)?;
            if value["fingerprint"] == fingerprint && value["status"] == "failed" {
                let admitted: bool = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM runs WHERE json_valid(manifest) AND json_extract(manifest,'$.source.call_counting.request_key')=?1)",
                    [key], |row| row.get(0),
                )?;
                if !admitted {
                    value["status"] = json!("reserved");
                    write_document(&tx, "call-count-requests", key, &value)?;
                    tx.commit()?;
                    return Ok((true, value));
                }
            }
            return Ok((false, value));
        }
        let value = json!({"run_id":parent,"fingerprint":fingerprint,"status":"reserved","created_at_us":crate::model::now_us()});
        write_document(&tx, "call-count-requests", key, &value)?;
        tx.commit()?;
        Ok((true, value))
    }

    pub(crate) fn latest_counting_run(&self, parent: &str) -> Result<Option<String>> {
        Ok(self.connection()?.query_row(
            "SELECT id FROM runs WHERE json_valid(manifest) AND json_extract(manifest,'$.source.call_counting.parent_run_id')=?1 ORDER BY started_at_us DESC,id DESC LIMIT 1",
            [parent], |row| row.get(0),
        ).optional()?)
    }

    pub(crate) fn counting_run_for_request(&self, key: &str) -> Result<Option<String>> {
        Ok(self.connection()?.query_row(
            "SELECT id FROM runs WHERE json_valid(manifest) AND json_extract(manifest,'$.source.call_counting.request_key')=?1 LIMIT 1",
            params![key], |row| row.get(0),
        ).optional()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::runs::Run;

    fn fixture(store: &Store) {
        store
            .save_run(&Run {
                id: "parent".into(),
                name: "parent".into(),
                status: "completed".into(),
                command: vec![],
                cwd: ".".into(),
                runtime: "none".into(),
                started_at_us: 1,
                finished_at_us: Some(2),
                exit_code: Some(0),
                diagnostics: vec![],
                source: json!({}),
                correctness: json!({}),
            })
            .unwrap();
    }

    #[test]
    fn definitive_pre_admission_failure_can_retry_but_an_unknown_reservation_cannot() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(root.path()).unwrap();
        fixture(&store);
        let (first, mut value) = store.claim_count_request("key", "same", "parent").unwrap();
        assert!(first);
        assert!(
            !store
                .claim_count_request("key", "same", "parent")
                .unwrap()
                .0
        );
        value["status"] = json!("failed");
        store
            .save_document("call-count-requests", "key", &value)
            .unwrap();
        assert!(
            store
                .claim_count_request("key", "same", "parent")
                .unwrap()
                .0
        );
        assert!(
            !store
                .claim_count_request("key", "same", "parent")
                .unwrap()
                .0
        );
        assert_eq!(
            store
                .claim_count_request("key", "different", "parent")
                .unwrap()
                .1["fingerprint"],
            "same"
        );
    }

    #[test]
    fn competing_retries_claim_one_failed_request_and_preserve_parent_ownership() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(root.path()).unwrap();
        fixture(&store);
        let (_, mut value) = store.claim_count_request("key", "same", "parent").unwrap();
        value["status"] = json!("failed");
        store
            .save_document("call-count-requests", "key", &value)
            .unwrap();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
        let mut threads = vec![];
        for _ in 0..2 {
            let s = store.clone();
            let b = barrier.clone();
            threads.push(std::thread::spawn(move || {
                b.wait();
                s.claim_count_request("key", "same", "parent").unwrap().0
            }));
        }
        barrier.wait();
        assert_eq!(
            threads
                .into_iter()
                .map(|t| usize::from(t.join().unwrap()))
                .sum::<usize>(),
            1
        );
        assert!(
            store
                .document_for_run("call-count-requests", "key", "parent")
                .unwrap()
                .is_some()
        );
    }
}
