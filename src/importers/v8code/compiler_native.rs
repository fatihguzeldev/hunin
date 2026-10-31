//! ARM64 identity checks between compiler, published relocation log and LL bytes.
use std::collections::{BTreeMap, BTreeSet};

pub(super) struct Published {
    pub address: u64,
    pub function_index: u64,
    pub words: BTreeMap<u64, u32>,
    pub relocations: BTreeSet<u64>,
    pub body_size: usize,
    pub instruction_size: usize,
}
pub(super) fn words(text: &str) -> BTreeMap<u64, u32> {
    text.lines()
        .filter_map(|line| {
            let fields = line.split_whitespace().collect::<Vec<_>>();
            if fields.len() < 3 || !fields[0].starts_with("0x") || fields[2].len() != 8 {
                return None;
            }
            Some((
                u64::from_str_radix(fields[1], 16).ok()?,
                u32::from_str_radix(fields[2], 16).ok()?,
            ))
        })
        .collect()
}
pub(super) fn published(text: &str) -> Vec<Published> {
    text.split("--- WebAssembly code ---")
        .skip(1)
        .filter_map(|section| {
            let section = section.split("--- End code ---").next()?;
            let compiler = section
                .lines()
                .find_map(|line| line.strip_prefix("compiler: "))?;
            if !compiler.to_ascii_lowercase().contains("turbofan") {
                return None;
            }
            let function_index = section
                .lines()
                .find_map(|line| line.strip_prefix("index: "))?
                .parse()
                .ok()?;
            let size = |prefix: &str| {
                section
                    .lines()
                    .find_map(|line| line.strip_prefix(prefix))?
                    .split(|ch: char| !ch.is_ascii_digit())
                    .next()?
                    .parse::<usize>()
                    .ok()
            };
            let body_size = size("Body (size = ")?;
            let instruction_size = size("Instructions (size = ")?;
            if instruction_size == 0
                || !instruction_size.is_multiple_of(4)
                || body_size < instruction_size
            {
                return None;
            }
            let rows = words(section);
            if rows.len().checked_mul(4) != Some(instruction_size) {
                return None;
            }
            let address = section.lines().find_map(|line| {
                let fields = line.split_whitespace().collect::<Vec<_>>();
                (fields.len() >= 3 && fields[1] == "0" && fields[2].len() == 8)
                    .then(|| u64::from_str_radix(fields[0].trim_start_matches("0x"), 16).ok())
                    .flatten()
            })?;
            let relocations = section
                .split("RelocInfo")
                .nth(1)
                .unwrap_or("")
                .lines()
                .filter_map(|line| {
                    if !line.contains("internal wasm call") && !line.contains("wasm stub call") {
                        return None;
                    }
                    u64::from_str_radix(
                        line.split_whitespace().next()?.trim_start_matches("0x"),
                        16,
                    )
                    .ok()?
                    .checked_sub(address)
                })
                .collect();
            Some(Published {
                address,
                function_index,
                words: rows,
                relocations,
                body_size,
                instruction_size,
            })
        })
        .collect()
}
pub(super) fn relocated_words_match(
    temporary: &BTreeMap<u64, u32>,
    published: &BTreeMap<u64, u32>,
    relocations: &BTreeSet<u64>,
    bytes: &[u8],
) -> bool {
    if temporary.is_empty()
        || temporary.len() != published.len()
        || published.len().checked_mul(4) != Some(bytes.len())
    {
        return false;
    }
    for (index, (&offset, &word)) in published.iter().enumerate() {
        if offset != index as u64 * 4 {
            return false;
        }
        let Some(before) = temporary.get(&offset) else {
            return false;
        };
        let Some(raw) = bytes.get(offset as usize..offset as usize + 4) else {
            return false;
        };
        if raw != word.to_le_bytes() {
            return false;
        }
        if *before != word
            && (!relocations.contains(&offset)
                || before & 0xfc000000 != 0x94000000
                || word & 0xfc000000 != 0x94000000)
        {
            return false;
        }
    }
    true
}
