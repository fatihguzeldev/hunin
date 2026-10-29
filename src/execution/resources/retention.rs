use serde::Serialize;
use serde_json::Value;
use std::io::{self, Write};

#[derive(Clone, Copy, Serialize)]
struct Limits {
    rows: usize,
    serialized_bytes: u64,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            rows: 100_000,
            serialized_bytes: 32 * 1024 * 1024,
        }
    }
}

#[derive(Serialize)]
pub(in crate::execution) struct Report {
    name: &'static str,
    limits: Limits,
    received_rows: u64,
    retained_rows: u64,
    retained_serialized_bytes: u64,
    pub discarded_rows: u64,
    discarded_serialized_bytes: u64,
    pub truncated: bool,
    first_exceeded: Option<&'static str>,
    scope: &'static str,
    limitations: &'static str,
}

pub(in crate::execution) struct Retention {
    rows: Vec<Value>,
    report: Report,
}
impl Default for Retention {
    fn default() -> Self {
        Self::with_limits(Limits::default())
    }
}
impl Retention {
    fn with_limits(limits: Limits) -> Self {
        Self {
            rows: Vec::new(),
            report: Report {
                name: "resource-retention-v1",
                limits,
                received_rows: 0,
                retained_rows: 0,
                retained_serialized_bytes: 0,
                discarded_rows: 0,
                discarded_serialized_bytes: 0,
                truncated: false,
                first_exceeded: None,
                scope: "retained prefix of received resource observation rows; serialized JSON bytes summed per row",
                limitations: "Discarded rows do not quantify lost CPU weights or unknown OS observation gaps; these limits do not bound sysinfo discovery, queued batches or whole-process heap usage",
            },
        }
    }
    pub fn extend(&mut self, rows: Vec<Value>) {
        for row in rows {
            let mut size = SerializedSize(0);
            serde_json::to_writer(&mut size, &row)
                .expect("serializing a JSON value into an infallible size counter");
            self.report.received_rows += 1;
            if !self.report.truncated {
                if self.rows.len() >= self.report.limits.rows {
                    self.report.truncated = true;
                    self.report.first_exceeded = Some("rows");
                } else if size.0
                    > self
                        .report
                        .limits
                        .serialized_bytes
                        .saturating_sub(self.report.retained_serialized_bytes)
                {
                    self.report.truncated = true;
                    self.report.first_exceeded = Some("serialized_bytes");
                }
            }
            if self.report.truncated {
                self.report.discarded_rows += 1;
                self.report.discarded_serialized_bytes += size.0;
                continue;
            }
            self.report.retained_rows += 1;
            self.report.retained_serialized_bytes += size.0;
            self.rows.push(row);
        }
    }
    pub fn finish(self) -> (Vec<Value>, Report) {
        (self.rows, self.report)
    }
}
struct SerializedSize(u64);
impl Write for SerializedSize {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0 += bytes.len() as u64;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn exact_limits_keep_bytes_and_one_over_freezes_the_prefix() {
        let rows = vec![json!({"value":"ç\n\""}), json!({"value":42})];
        let bytes = rows
            .iter()
            .map(|row| serde_json::to_vec(row).unwrap().len() as u64)
            .sum();
        let mut retained = Retention::with_limits(Limits {
            rows: 2,
            serialized_bytes: bytes,
        });
        retained.extend(rows.clone());
        assert!(!retained.report.truncated);
        assert_eq!(retained.report.retained_serialized_bytes, bytes);
        retained.extend(vec![json!({"value":3}), json!({"value":4})]);
        let (actual, report) = retained.finish();
        assert_eq!(actual, rows);
        assert!(report.truncated);
        assert_eq!(report.discarded_rows, 2);
        assert_eq!(report.received_rows, 4);
    }
    #[test]
    fn byte_cap_discards_later_small_rows_even_if_they_would_fit() {
        let first = json!({"value":1});
        let large = json!({"value":"large observation"});
        let small = json!(0);
        let bytes = serde_json::to_vec(&first).unwrap().len() as u64 + 2;
        let mut retained = Retention::with_limits(Limits {
            rows: 10,
            serialized_bytes: bytes,
        });
        retained.extend(vec![first.clone(), large.clone(), small.clone()]);
        let (actual, report) = retained.finish();
        assert_eq!(actual, vec![first]);
        assert_eq!(report.first_exceeded, Some("serialized_bytes"));
        assert_eq!(report.discarded_rows, 2);
        assert_eq!(
            report.discarded_serialized_bytes,
            serde_json::to_vec(&large).unwrap().len() as u64
                + serde_json::to_vec(&small).unwrap().len() as u64
        );
    }
}
