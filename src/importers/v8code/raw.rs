use anyhow::{Context, Result, ensure};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug)]
pub(super) struct Code {
    pub address: u64,
    pub size: u64,
    pub name: String,
    pub start_us: u64,
    pub end_us: u64,
    pub start_censored: bool,
    pub end_censored: bool,
    pub generation: usize,
    pub create_line: usize,
    pub bytecode: bool,
    pub origin: usize,
}
#[derive(Debug)]
pub(super) struct Frame {
    pub address: u64,
    pub code: Option<usize>,
    pub arm64_callsite_code: Option<usize>,
}
#[derive(Debug)]
pub(super) struct Tick {
    pub timestamp_us: u64,
    pub line: usize,
    pub frames: Vec<Frame>,
    pub overflow: bool,
}
#[derive(Debug)]
pub(super) struct Log {
    pub version: String,
    pub codes: Vec<Code>,
    pub ticks: Vec<Tick>,
    pub diagnostics: Vec<String>,
}
fn integer(value: &str) -> Result<u64> {
    let n = value.parse::<u64>().context("invalid V8 integer")?;
    ensure!(
        n <= 9_007_199_254_740_991,
        "V8 timestamp exceeds exact protocol range"
    );
    Ok(n)
}
fn address(value: &str) -> Result<u64> {
    u64::from_str_radix(value.strip_prefix("0x").context("V8 address needs 0x")?, 16)
        .context("invalid V8 address")
}
fn resolve(active: &BTreeMap<u64, usize>, codes: &[Code], pc: u64) -> Option<usize> {
    active
        .range(..=pc)
        .next_back()
        .and_then(|(_, &index)| (pc < codes[index].address + codes[index].size).then_some(index))
}
fn retire(active: &mut BTreeMap<u64, usize>, codes: &mut [Code], start: u64, end: u64, now: u64) {
    let mut hits = active
        .range(start..end)
        .map(|(&a, &i)| (a, i))
        .collect::<Vec<_>>();
    if let Some((&a, &i)) = active.range(..start).next_back()
        && codes[i].address + codes[i].size > start
    {
        hits.push((a, i));
    }
    for (a, index) in hits {
        active.remove(&a);
        codes[index].end_us = now.saturating_add(1).max(codes[index].start_us + 1);
        codes[index].end_censored = true;
    }
}
pub(super) enum Visit {
    Record,
    Frame,
}

#[cfg(test)]
pub(super) fn parse(text: &str) -> Result<Log> {
    parse_controlled(text, |_| Ok(()))
}

pub(super) fn parse_controlled(
    text: &str,
    mut visit: impl FnMut(Visit) -> Result<()>,
) -> Result<Log> {
    let mut result = Log {
        version: String::new(),
        codes: vec![],
        ticks: vec![],
        diagnostics: vec![],
    };
    let mut active = BTreeMap::new();
    let mut unknown = BTreeSet::new();
    let mut now = 0;
    let mut frames_seen = 0u64;
    for (line_index, line) in text.lines().enumerate() {
        visit(Visit::Record)?;
        ensure!(line_index < 2_000_000, "V8 log record limit exceeded");
        if line.is_empty() {
            continue;
        }
        let mut values = line.split(',');
        let fields = values.by_ref().take(8192 + 8).collect::<Vec<_>>();
        ensure!(values.next().is_none(), "V8 record field limit exceeded");
        let line_number = line_index + 1;
        let field = |index| {
            fields
                .get(index)
                .copied()
                .context("truncated V8 log record")
        };
        match fields[0] {
            "v8-version" => {
                ensure!(
                    fields.len() >= 6 && result.version.is_empty(),
                    "invalid V8 version header"
                );
                result.version = format!(
                    "{}.{}.{}.{}{}",
                    field(1)?,
                    field(2)?,
                    field(3)?,
                    field(4)?,
                    field(5)?
                );
            }
            "code-creation" => {
                let time = integer(field(3)?)?;
                now = now.max(time);
                let start = address(field(4)?)?;
                let size = integer(field(5)?)?;
                let end = start
                    .checked_add(size)
                    .context("V8 code address range overflows")?;
                ensure!(
                    size > 0 && result.codes.len() < 100_000,
                    "invalid or excessive V8 code range"
                );
                retire(&mut active, &mut result.codes, start, end, now);
                let index = result.codes.len();
                result.codes.push(Code {
                    address: start,
                    size,
                    name: field(6)?.to_owned(),
                    start_us: time,
                    end_us: time.saturating_add(1),
                    start_censored: false,
                    end_censored: true,
                    generation: index,
                    create_line: line_number,
                    bytecode: fields.get(8) == Some(&"~"),
                    origin: index,
                });
                active.insert(start, index);
            }
            "code-move" => {
                let from = address(field(1)?)?;
                let to = address(field(2)?)?;
                if let Some(index) = active.remove(&from) {
                    let size = result.codes[index].size;
                    let end = to
                        .checked_add(size)
                        .context("moved V8 code range overflows")?;
                    result.codes[index].end_us = (now + 1).max(result.codes[index].start_us + 1);
                    let next = Code {
                        address: to,
                        size,
                        name: result.codes[index].name.clone(),
                        start_us: now,
                        end_us: now + 1,
                        start_censored: true,
                        end_censored: true,
                        generation: result.codes.len(),
                        create_line: line_number,
                        bytecode: result.codes[index].bytecode,
                        origin: result.codes[index].origin,
                    };
                    retire(&mut active, &mut result.codes, to, end, now);
                    active.insert(to, next.generation);
                    result.codes.push(next);
                } else {
                    unknown.insert("move without captured creation".to_owned());
                }
            }
            "code-delete" => {
                let start = address(field(1)?)?;
                if let Some(index) = active.remove(&start) {
                    result.codes[index].end_us = (now + 1).max(result.codes[index].start_us + 1);
                }
            }
            "tick" => {
                ensure!(result.ticks.len() < 100_000, "V8 tick limit exceeded");
                let pc = address(field(1)?)?;
                let timestamp_us = integer(field(2)?)?;
                now = now.max(timestamp_us);
                field(5)?.parse::<i32>().context("invalid V8 VM state")?;
                frames_seen += 1;
                ensure!(
                    frames_seen <= 2_000_000,
                    "V8 aggregate parsed-frame limit exceeded"
                );
                visit(Visit::Frame)?;
                let mut frames = vec![Frame {
                    address: pc,
                    code: resolve(&active, &result.codes, pc),
                    arm64_callsite_code: None,
                }];
                let mut overflow = false;
                for raw in fields.iter().skip(6) {
                    if *raw == "overflow" {
                        overflow = true;
                        continue;
                    }
                    ensure!(frames.len() < 8192, "V8 stack limit exceeded");
                    frames_seen += 1;
                    ensure!(
                        frames_seen <= 2_000_000,
                        "V8 aggregate parsed-frame limit exceeded"
                    );
                    visit(Visit::Frame)?;
                    let pc = address(raw)?;
                    frames.push(Frame {
                        address: pc,
                        code: resolve(&active, &result.codes, pc),
                        arm64_callsite_code: pc
                            .checked_sub(4)
                            .and_then(|pc| resolve(&active, &result.codes, pc)),
                    });
                }
                result.ticks.push(Tick {
                    timestamp_us,
                    line: line_number,
                    frames,
                    overflow,
                });
            }
            "v8-platform" | "profiler" | "shared-library" | "shared-library-end"
            | "heap-capacity" | "heap-available" | "new" | "delete" | "script-source"
            | "code-source-info" | "code-disassemble" | "sfi-move" => {}
            name => {
                if unknown.len() < 128 {
                    unknown.insert(name.to_owned());
                }
            }
        }
    }
    ensure!(!result.version.is_empty(), "V8 log version header missing");
    for index in active.into_values() {
        result.codes[index].end_us = (now + 1).max(result.codes[index].start_us + 1);
    }
    result.diagnostics.extend(
        unknown
            .into_iter()
            .map(|name| format!("V8 record not interpreted: {name}")),
    );
    Ok(result)
}
