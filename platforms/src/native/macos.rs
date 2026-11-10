use super::{AggregateStack, Frame, Image, Language, Profile, SourceLocation};
use crate::{
    process::{Error, ErrorKind, Result},
    tools,
};
use regex::Regex;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Command,
    sync::OnceLock,
    time::Duration,
};

fn uuid_regex() -> &'static Regex {
    static PATTERN: OnceLock<Regex> = OnceLock::new();
    PATTERN.get_or_init(|| {
        Regex::new(
            r"^[[:xdigit:]]{8}-[[:xdigit:]]{4}-[[:xdigit:]]{4}-[[:xdigit:]]{4}-[[:xdigit:]]{12}$",
        )
        .unwrap()
    })
}
pub fn parse_uuids(text: &str) -> Result<Vec<(String, String)>> {
    let mut result = Vec::new();
    for line in text.lines() {
        let fields: Vec<_> = line.split_whitespace().collect();
        if fields.len() < 4
            || fields[0] != "UUID:"
            || !uuid_regex().is_match(fields[1])
            || !fields[2].starts_with('(')
            || !fields[2].ends_with(')')
        {
            return Err(Error::invalid("invalid dwarfdump uuid record"));
        }
        result.push((
            fields[1].to_ascii_uppercase(),
            fields[2][1..fields[2].len() - 1].to_owned(),
        ));
    }
    if result.is_empty() {
        return Err(Error::invalid("no Mach-O uuid records"));
    }
    Ok(result)
}
pub fn parse_atos_location(text: &str) -> Option<SourceLocation> {
    let (_, suffix) = text.trim().rsplit_once(") (")?;
    let (file, line) = suffix.strip_suffix(')')?.rsplit_once(':')?;
    let line: u32 = line.parse().ok()?;
    if line == 0 || file.is_empty() {
        return None;
    }
    Some(SourceLocation {
        file: file.into(),
        line,
        column: None,
    })
}

pub fn parse_sample(text: &str, language: Language) -> Result<Profile> {
    if text.len() > 64 * 1024 * 1024 {
        return Err(Error::new(
            ErrorKind::LimitExceeded,
            "sample report exceeds 64 mib",
        ));
    }
    static IMAGE: OnceLock<Regex> = OnceLock::new();
    static STACK: OnceLock<Regex> = OnceLock::new();
    static THREAD: OnceLock<Regex> = OnceLock::new();
    let image_re = IMAGE.get_or_init(|| {
        Regex::new(r"^\s*(0x[[:xdigit:]]+)\s+-\s+(0x[[:xdigit:]]+)\s+.*?<([[:xdigit:]-]+)>\s+(.+)$")
            .unwrap()
    });
    let stack_re = STACK.get_or_init(|| Regex::new(r"^(?P<indent>[ +|:!]*)?(?P<count>[0-9]+) (?P<function>.+?)\s+\(in (?P<module>.+?)\)(?: \+ [^\[]+)?\s+\[(?P<addresses>[^\]]+)\](?:\s+(?P<source>.+))?$").unwrap());
    let thread_re = THREAD.get_or_init(|| Regex::new(r"^\s*[0-9]+ Thread_([0-9]+).*$").unwrap());
    let mut header = BTreeMap::new();
    let mut images = Vec::new();
    let mut aggregates = Vec::new();
    let mut in_graph = false;
    let mut in_images = false;
    let mut thread_id = None;
    let mut parents: Vec<(usize, u64)> = Vec::new();
    for line in text.lines() {
        if line.len() > 65_536 {
            return Err(Error::new(
                ErrorKind::LimitExceeded,
                "sample line exceeds 64 kib",
            ));
        }
        if line == "Call graph:" {
            in_graph = true;
            continue;
        }
        if line.starts_with("Total number in stack") || line.starts_with("Sort by top") {
            in_graph = false;
        }
        if line == "Binary Images:" {
            in_graph = false;
            in_images = true;
            continue;
        }
        if in_images {
            if line.trim().is_empty() {
                continue;
            }
            let captures = image_re.captures(line).ok_or_else(|| {
                Error::new(
                    ErrorKind::UnsupportedFormat,
                    "unrecognized sample image row",
                )
            })?;
            if !uuid_regex().is_match(&captures[3]) {
                return Err(Error::invalid("invalid sample image uuid"));
            }
            let load = hex(&captures[1])?;
            let end = hex(&captures[2])?;
            if end < load {
                return Err(Error::invalid("sample image range is reversed"));
            }
            images.push(Image {
                path: PathBuf::from(captures[4].trim()),
                architecture: header.get("Code Type").cloned().unwrap_or_default(),
                identity_kind: "mach_uuid".into(),
                identity: captures[3].to_ascii_uppercase(),
                load_address: load,
                file_offset: 0,
                mapped_virtual_address: None,
                end_address: Some(end),
            });
            if images.len() > 100_000 {
                return Err(Error::new(
                    ErrorKind::LimitExceeded,
                    "too many sample images",
                ));
            }
        } else if in_graph {
            if line.trim().is_empty() {
                continue;
            }
            if let Some(captures) = thread_re.captures(line) {
                thread_id = Some(
                    captures[1]
                        .parse()
                        .map_err(|_| Error::invalid("sample thread id overflow"))?,
                );
                parents.clear();
                continue;
            }
            let captures = stack_re.captures(line).ok_or_else(|| {
                Error::new(
                    ErrorKind::UnsupportedFormat,
                    format!("unrecognized sample callgraph row: {line}"),
                )
            })?;
            let indent = captures
                .name("indent")
                .map(|m| m.as_str().len())
                .unwrap_or(0);
            while parents.last().is_some_and(|(depth, _)| *depth >= indent) {
                parents.pop();
            }
            let mut addresses = Vec::new();
            let mut truncated = false;
            for address in captures["addresses"].split(',') {
                if address.trim() == "..." {
                    truncated = true;
                } else {
                    addresses.push(hex(address.trim())?);
                }
            }
            if addresses.is_empty() {
                return Err(Error::invalid("sample stack has no address"));
            }
            let source = captures
                .name("source")
                .and_then(|value| value.as_str().rsplit_once(':'))
                .and_then(|(file, line)| {
                    Some(SourceLocation {
                        file: file.into(),
                        line: line.parse().ok()?,
                        column: None,
                    })
                });
            let id = aggregates.len() as u64;
            aggregates.push(AggregateStack {
                id,
                parent: parents.last().map(|(_, id)| *id),
                thread_id,
                count: captures["count"]
                    .parse()
                    .map_err(|_| Error::invalid("sample count overflow"))?,
                frame: Frame {
                    address: addresses[0],
                    alternate_addresses: addresses[1..].to_vec(),
                    addresses_truncated: truncated,
                    image: None,
                    function: Some(captures["function"].trim().into()),
                    reported_function: Some(captures["function"].trim().into()),
                    inline_sites: Vec::new(),
                    source,
                    resolution: "producer_reported".into(),
                },
            });
            parents.push((indent, id));
            if aggregates.len() > 1_000_000 || parents.len() > 1024 {
                return Err(Error::new(
                    ErrorKind::LimitExceeded,
                    "sample callgraph exceeds record/depth budget",
                ));
            }
        } else if let Some((key, value)) = line.split_once(':')
            && !key.trim().is_empty()
            && header.len() < 256
        {
            header.insert(key.trim().to_owned(), value.trim().to_owned());
        }
    }
    if header.get("Report Version").map(String::as_str) != Some("7") {
        return Err(Error::new(
            ErrorKind::UnsupportedFormat,
            "only sample report version 7 is implemented",
        ));
    }
    if aggregates.is_empty() || images.is_empty() {
        return Err(Error::invalid(
            "sample report lacks callgraph or image records",
        ));
    }
    for node in &mut aggregates {
        node.frame.image = images.iter().position(|image| {
            image.load_address <= node.frame.address
                && image
                    .end_address
                    .is_some_and(|end| node.frame.address <= end)
        });
    }
    Ok(Profile { schema: "macos.sample.aggregate.v7".into(), producer: "sample".into(), language, images, samples: Vec::new(), aggregates, clock: None,ticks_per_second:1_000_000_000, period: None, start: None, end: None, issues: vec!["aggregate thread-stack counts have no individual timestamps or on-cpu attribution; collapsed address lists may be truncated by the producer".into()], metadata: serde_json::to_value(header).map_err(|e| Error::invalid(e.to_string()))? })
}

fn hex(value: &str) -> Result<u64> {
    u64::from_str_radix(value.strip_prefix("0x").unwrap_or(value), 16)
        .map_err(|_| Error::invalid("invalid native address"))
}

pub fn verify_mach_build(
    binary: &Path,
    debug: &Path,
    expected_uuid: &str,
    architecture: &str,
    temporary: &Path,
) -> Result<()> {
    if !uuid_regex().is_match(expected_uuid) {
        return Err(Error::invalid("invalid expected Mach-O uuid"));
    }
    for path in [binary, debug] {
        let result = tools::require_success(tools::run(
            Command::new("xcrun")
                .arg("dwarfdump")
                .arg("--uuid")
                .arg(path),
            temporary,
            Duration::from_secs(10),
            65_536,
        )?)?;
        if !parse_uuids(&result.stdout)?.iter().any(|(uuid, arch)| {
            uuid.eq_ignore_ascii_case(expected_uuid) && arch.eq_ignore_ascii_case(architecture)
        }) {
            return Err(Error::new(
                ErrorKind::SymbolMismatch,
                format!(
                    "binary/debug uuid or architecture mismatch: {}",
                    path.display()
                ),
            ));
        }
    }
    Ok(())
}

pub fn resolve_atos(
    image: &Image,
    binary: &Path,
    debug: &Path,
    addresses: &[u64],
    temporary: &Path,
) -> Result<Vec<super::InlineSite>> {
    let architecture = if image
        .architecture
        .replace('-', "")
        .eq_ignore_ascii_case("ARM64")
    {
        "arm64"
    } else if image.architecture.eq_ignore_ascii_case("X86-64") {
        "x86_64"
    } else {
        image.architecture.as_str()
    };
    verify_mach_build(binary, debug, &image.identity, architecture, temporary)?;
    if addresses.is_empty() || addresses.len() > 4096 {
        return Err(Error::invalid(
            "atos address batch must have 1..4096 entries",
        ));
    }
    let mut command = Command::new("atos");
    command
        .arg("-o")
        .arg(debug)
        .arg("-arch")
        .arg(architecture)
        .arg("-l")
        .arg(format!("0x{:x}", image.load_address))
        .arg("-fullPath");
    for address in addresses {
        command.arg(format!("0x{address:x}"));
    }
    let result = tools::require_success(tools::run(
        &mut command,
        temporary,
        Duration::from_secs(15),
        4 * 1024 * 1024,
    )?)?;
    let lines: Vec<_> = result.stdout.lines().collect();
    if lines.len() != addresses.len() {
        return Err(Error::new(
            ErrorKind::UnsupportedFormat,
            "atos returned an unexpected number of address results",
        ));
    }
    let debug_lines = tools::require_success(tools::run(
        Command::new("xcrun")
            .arg("dwarfdump")
            .arg("--debug-line=0")
            .arg(debug),
        temporary,
        Duration::from_secs(10),
        4 * 1024 * 1024,
    )?)?;
    let has_lines = debug_lines.stdout.contains("Line table prologue:");
    Ok(lines
        .into_iter()
        .map(|line| super::InlineSite {
            function: line
                .split_once(" (in ")
                .map(|(name, _)| name.to_string())
                .filter(|name| !name.starts_with("0x") && name != "???"),
            source: if has_lines {
                parse_atos_location(line)
            } else {
                None
            },
        })
        .collect())
}

pub fn resolve_profile(
    profile: &mut Profile,
    binary: &Path,
    debug: &Path,
    uuid: &str,
    temporary: &Path,
) -> Result<()> {
    let image_index = profile
        .images
        .iter()
        .position(|image| image.identity.eq_ignore_ascii_case(uuid))
        .ok_or_else(|| {
            Error::new(
                ErrorKind::SymbolMismatch,
                "expected executable uuid is absent from recording",
            )
        })?;
    let image = &profile.images[image_index];
    let indices: Vec<_> = profile
        .aggregates
        .iter()
        .enumerate()
        .filter(|(_, node)| node.frame.image == Some(image_index))
        .map(|(i, _)| i)
        .collect();
    for chunk in indices.chunks(4096) {
        let addresses: Vec<_> = chunk
            .iter()
            .map(|&i| profile.aggregates[i].frame.address)
            .collect();
        let resolved = resolve_atos(image, binary, debug, &addresses, temporary)?;
        for (&index, resolved) in chunk.iter().zip(resolved) {
            let frame = &mut profile.aggregates[index].frame;
            frame.source = resolved.source;
            frame.function = resolved.function;
            frame.resolution = if frame.source.is_some() {
                "matched_build_primary_address"
            } else {
                "matched_build_no_source"
            }
            .into();
        }
    }
    if indices.is_empty() {
        return Err(Error::new(
            ErrorKind::CollectorFailure,
            "recording has no frames in the expected image",
        ));
    }
    Ok(())
}
