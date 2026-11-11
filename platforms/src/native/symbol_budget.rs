use super::Profile;
use crate::process::{Error, ErrorKind, Result};
use serde::Serialize;
use std::io::Write;

const MAX_PROFILE_BYTES: usize = 64 * 1024 * 1024;

pub(super) fn encoded_size(value: &impl Serialize) -> Result<usize> {
    struct Counter {
        bytes: usize,
        exceeded: bool,
    }
    impl Write for Counter {
        fn write(&mut self, input: &[u8]) -> std::io::Result<usize> {
            let Some(bytes) = self.bytes.checked_add(input.len()) else {
                self.exceeded = true;
                return Err(std::io::Error::other("symbol size overflow"));
            };
            if bytes > MAX_PROFILE_BYTES {
                self.exceeded = true;
                return Err(std::io::Error::other("resolved profile exceeds budget"));
            }
            self.bytes = bytes;
            Ok(input.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut counter = Counter {
        bytes: 0,
        exceeded: false,
    };
    serde_json::to_writer(&mut counter, value).map_err(|error| {
        if counter.exceeded {
            limit()
        } else {
            Error::invalid(error.to_string())
        }
    })?;
    Ok(counter.bytes)
}

pub(super) struct SymbolBudget {
    bytes: usize,
}

impl SymbolBudget {
    pub fn new(profile: &Profile) -> Result<Self> {
        Ok(Self {
            bytes: encoded_size(profile)?,
        })
    }

    pub fn replace(&mut self, old: &impl Serialize, new: &impl Serialize) -> Result<()> {
        self.replace_repeated(encoded_size(old)?, new, 1)
    }

    pub fn replace_repeated(
        &mut self,
        old_bytes: usize,
        new: &impl Serialize,
        repetitions: usize,
    ) -> Result<()> {
        let replacement = encoded_size(new)?
            .checked_mul(repetitions)
            .ok_or_else(limit)?;
        let bytes = self
            .bytes
            .checked_sub(old_bytes)
            .and_then(|bytes| bytes.checked_add(replacement))
            .filter(|bytes| *bytes <= MAX_PROFILE_BYTES)
            .ok_or_else(limit)?;
        self.bytes = bytes;
        Ok(())
    }
}

fn limit() -> Error {
    Error::new(
        ErrorKind::LimitExceeded,
        "materialized native profile exceeds 64 mib symbol budget",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::{Frame, Language, Sample};
    use serde_json::json;

    fn profile() -> Profile {
        Profile {
            schema: "pressure".into(),
            producer: "synthetic budget control".into(),
            language: Language::C,
            images: vec![],
            samples: vec![
                Sample {
                    instruction_pointer: 1,
                    producer_fields: json!({}),
                    process_id: 1,
                    timestamp: 0,
                    thread_id: 1,
                    cpu: None,
                    weight: 1,
                    weight_unit: "samples".into(),
                    frames: vec![Frame {
                        address: 1,
                        alternate_addresses: vec![],
                        addresses_truncated: false,
                        image: None,
                        function: None,
                        reported_function: None,
                        inline_sites: vec![],
                        source: None,
                        resolution: "unresolved".into(),
                    }],
                };
                1100
            ],
            aggregates: vec![],
            clock: None,
            ticks_per_second: 1_000_000_000,
            period: None,
            start: None,
            end: None,
            issues: vec![],
            metadata: json!({}),
        }
    }

    #[test]
    fn repeated_symbols_are_rejected_before_materializing_their_copies() {
        let profile = profile();
        let mut budget = SymbolBudget::new(&profile).unwrap();
        let initial = budget.bytes;
        let old = (&None::<String>, &None::<String>);
        let name = Some("f".repeat(65536));
        let new = (&name, &None::<String>);
        let error = budget
            .replace_repeated(encoded_size(&old).unwrap() * 1100, &new, 1100)
            .unwrap_err();
        assert_eq!(error.kind, ErrorKind::LimitExceeded);
        assert_eq!(budget.bytes, initial);
        assert!(
            profile
                .samples
                .iter()
                .all(|sample| sample.frames[0].function.is_none())
        );
    }

    #[test]
    fn escaped_symbols_and_metadata_are_counted_without_serialized_buffers() {
        let mut profile = profile();
        profile.samples.truncate(2);
        let mut budget = SymbolBudget::new(&profile).unwrap();
        let new = Some("quoted\"\nname".to_owned());
        budget
            .replace_repeated(encoded_size(&None::<String>).unwrap() * 2, &new, 2)
            .unwrap();
        for sample in &mut profile.samples {
            sample.frames[0].function = new.clone();
        }
        let metadata = json!({"symbolizer":"selected tool"});
        budget.replace(&profile.metadata, &metadata).unwrap();
        profile.metadata = metadata;
        assert_eq!(budget.bytes, serde_json::to_vec(&profile).unwrap().len());
    }
}
