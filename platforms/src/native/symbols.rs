use super::{Frame, Image, SourceLocation};
use crate::{
    process::{Error, ErrorKind, Result},
    tools,
};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::Path,
    process::Command,
    time::Duration,
};

pub fn elf_build_id(path: &Path) -> Result<(String, u16)> {
    let mut file = File::open(path)?;
    let mut header = [0_u8; 64];
    file.read_exact(&mut header)?;
    if &header[..4] != b"\x7fELF" || header[4] != 2 || header[5] != 1 {
        return Err(Error::new(
            ErrorKind::UnsupportedFormat,
            "only 64-bit little-endian ELF build identities are implemented",
        ));
    }
    let kind = u16::from_le_bytes(header[16..18].try_into().unwrap());
    let phoff = u64::from_le_bytes(header[32..40].try_into().unwrap());
    let size = u16::from_le_bytes(header[54..56].try_into().unwrap()) as u64;
    let count = u16::from_le_bytes(header[56..58].try_into().unwrap()) as u64;
    if !(56..=4096).contains(&size) || count > 4096 {
        return Err(Error::invalid("invalid ELF program table"));
    }
    for index in 0..count {
        file.seek(SeekFrom::Start(
            phoff
                .checked_add(
                    index
                        .checked_mul(size)
                        .ok_or_else(|| Error::invalid("ELF offset overflow"))?,
                )
                .ok_or_else(|| Error::invalid("ELF offset overflow"))?,
        ))?;
        let mut program = [0_u8; 56];
        file.read_exact(&mut program)?;
        if u32::from_le_bytes(program[..4].try_into().unwrap()) != 4 {
            continue;
        }
        let offset = u64::from_le_bytes(program[8..16].try_into().unwrap());
        let length = u64::from_le_bytes(program[32..40].try_into().unwrap());
        if length > 2 * 1024 * 1024 {
            return Err(Error::new(
                ErrorKind::LimitExceeded,
                "ELF notes exceed 2 mib",
            ));
        }
        file.seek(SeekFrom::Start(offset))?;
        let mut notes = vec![0; length as usize];
        file.read_exact(&mut notes)?;
        let mut cursor = 0;
        while cursor + 12 <= notes.len() {
            let names = u32::from_le_bytes(notes[cursor..cursor + 4].try_into().unwrap()) as usize;
            let data =
                u32::from_le_bytes(notes[cursor + 4..cursor + 8].try_into().unwrap()) as usize;
            let typ = u32::from_le_bytes(notes[cursor + 8..cursor + 12].try_into().unwrap());
            cursor += 12;
            let align = |n: usize| {
                n.checked_add(3)
                    .map(|n| n & !3)
                    .ok_or_else(|| Error::invalid("ELF note alignment overflow"))
            };
            let data_start = cursor
                .checked_add(align(names)?)
                .ok_or_else(|| Error::invalid("ELF note overflow"))?;
            let end = data_start
                .checked_add(align(data)?)
                .ok_or_else(|| Error::invalid("ELF note overflow"))?;
            if end > notes.len() {
                return Err(Error::invalid("truncated ELF note"));
            }
            if typ == 3 && names == 4 && &notes[cursor..cursor + 4] == b"GNU\0" {
                if data == 0 || data > 64 {
                    return Err(Error::invalid("unsupported ELF build-id size"));
                }
                return Ok((
                    notes[data_start..data_start + data]
                        .iter()
                        .map(|b| format!("{b:02x}"))
                        .collect(),
                    kind,
                ));
            }
            cursor = end;
        }
    }
    Err(Error::new(
        ErrorKind::SymbolMismatch,
        "ELF has no GNU build id",
    ))
}

pub fn resolve_elf(
    image: &Image,
    debug: &Path,
    frames: &mut [Frame],
    temporary: &Path,
) -> Result<()> {
    if image.identity_kind != "elf_build_id" || image.identity.is_empty() {
        return Err(Error::new(
            ErrorKind::SymbolMismatch,
            "recorded image has no usable ELF build id",
        ));
    }
    let (actual, kind) = elf_build_id(debug)?;
    if actual != image.identity {
        return Err(Error::new(
            ErrorKind::SymbolMismatch,
            "ELF debug/build-id mismatch",
        ));
    }
    let indices: Vec<_> = frames
        .iter()
        .enumerate()
        .filter(|(_, frame)| {
            frame.address >= image.load_address
                && image.end_address.is_some_and(|end| frame.address <= end)
        })
        .map(|(i, _)| i)
        .collect();
    for chunk in indices.chunks(1024) {
        let mut command = Command::new("llvm-symbolizer");
        command
            .args(["--output-style=JSON", "--inlining", "--demangle", "--obj"])
            .arg(debug);
        for &index in chunk {
            let frame = &frames[index];
            let address = if kind == 2 {
                frame.address
            } else {
                let offset = frame
                    .address
                    .checked_sub(image.load_address)
                    .and_then(|n| n.checked_add(image.file_offset))
                    .ok_or_else(|| Error::invalid("ELF address conversion overflow"))?;
                if let Some(virtual_address) = image.mapped_virtual_address {
                    virtual_address
                        .checked_add(frame.address - image.load_address)
                        .ok_or_else(|| Error::invalid("elf mapped virtual address overflow"))?
                } else {
                    elf_virtual_address(debug, offset, None)?
                }
            };
            command.arg(format!("0x{address:x}"));
        }
        let output = tools::require_success(tools::run(
            &mut command,
            temporary,
            Duration::from_secs(10),
            4 * 1024 * 1024,
        )?)?;
        let result: serde_json::Value = serde_json::from_str(&output.stdout)
            .map_err(|e| Error::new(ErrorKind::UnsupportedFormat, e.to_string()))?;
        let values = match &result {
            serde_json::Value::Array(array) if array.len() == chunk.len() => {
                array.iter().collect::<Vec<_>>()
            }
            serde_json::Value::Object(_) if chunk.len() == 1 => vec![&result],
            _ => {
                return Err(Error::new(
                    ErrorKind::UnsupportedFormat,
                    "llvm-symbolizer result count differs from input",
                ));
            }
        };
        for (&index, value) in chunk.iter().zip(values) {
            let symbols = value
                .get("Symbol")
                .and_then(|v| v.as_array())
                .ok_or_else(|| {
                    Error::new(
                        ErrorKind::UnsupportedFormat,
                        "llvm-symbolizer JSON has no Symbol array",
                    )
                })?;
            let frame = &mut frames[index];
            let Some(symbol) = symbols.first() else {
                frame.resolution = "unresolved".into();
                continue;
            };
            frame.function = symbol
                .get("FunctionName")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty() && *s != "??")
                .map(str::to_string);
            frame.source = source_location(symbol);
            frame.inline_sites = symbols
                .iter()
                .skip(1)
                .map(|symbol| super::InlineSite {
                    function: symbol
                        .get("FunctionName")
                        .and_then(|v| v.as_str())
                        .filter(|s| !s.is_empty() && *s != "??")
                        .map(str::to_string),
                    source: source_location(symbol),
                })
                .collect();
            frame.resolution = if frame.source.is_some() {
                "matched_build"
            } else {
                "matched_build_no_source"
            }
            .into();
        }
    }
    Ok(())
}

pub fn resolve_elf_profile(
    profile: &mut super::Profile,
    debug: &Path,
    temporary: &Path,
) -> Result<()> {
    let (identity, _) = elf_build_id(debug)?;
    let images: Vec<_> = profile
        .images
        .iter()
        .enumerate()
        .filter(|(_, image)| image.identity_kind == "elf_build_id" && image.identity == identity)
        .map(|(i, _)| i)
        .collect();
    if images.is_empty() {
        return Err(Error::new(
            ErrorKind::SymbolMismatch,
            "debug build id is absent from recorded images",
        ));
    }
    for image_index in images {
        let mut unique = std::collections::BTreeMap::new();
        for sample in &profile.samples {
            for frame in &sample.frames {
                if frame.image == Some(image_index) {
                    unique.entry(frame.address).or_insert_with(|| frame.clone());
                }
            }
        }
        let mut frames: Vec<_> = unique.into_values().collect();
        resolve_elf(&profile.images[image_index], debug, &mut frames, temporary)?;
        let resolved: std::collections::BTreeMap<_, _> = frames
            .into_iter()
            .map(|frame| (frame.address, frame))
            .collect();
        for sample in &mut profile.samples {
            for frame in &mut sample.frames {
                if frame.image == Some(image_index)
                    && let Some(symbol) = resolved.get(&frame.address)
                {
                    frame.function = symbol.function.clone();
                    frame.source = symbol.source.clone();
                    frame.inline_sites = symbol.inline_sites.clone();
                    frame.resolution = symbol.resolution.clone();
                }
            }
        }
    }
    let version = tools::require_success(tools::run(
        Command::new("llvm-symbolizer").arg("--version"),
        temporary,
        Duration::from_secs(10),
        65536,
    )?)?;
    profile.metadata["symbolizer"] = serde_json::json!({"version":version.stdout,"build_id":identity,"address_translation":"executable ELF PT_LOAD virtual address; captured mapping bases include host page alignment"});
    Ok(())
}

fn source_location(symbol: &serde_json::Value) -> Option<SourceLocation> {
    let file = symbol.get("FileName")?.as_str()?;
    let line = u32::try_from(symbol.get("Line")?.as_u64()?).ok()?;
    if file.is_empty() || file == "??" || line == 0 {
        return None;
    }
    Some(SourceLocation {
        file: file.into(),
        line,
        column: symbol
            .get("Column")
            .and_then(|v| v.as_u64())
            .and_then(|v| u32::try_from(v).ok()),
    })
}

pub fn pe_pdb_identity(path: &Path) -> Result<(String, u16)> {
    let mut file = File::open(path)?;
    let mut dos = [0u8; 64];
    file.read_exact(&mut dos)?;
    if &dos[..2] != b"MZ" {
        return Err(Error::new(ErrorKind::UnsupportedFormat, "not a pe image"));
    }
    let offset = u32::from_le_bytes(dos[60..64].try_into().unwrap()) as u64;
    file.seek(SeekFrom::Start(offset))?;
    let mut coff = [0u8; 24];
    file.read_exact(&mut coff)?;
    if &coff[..4] != b"PE\0\0" {
        return Err(Error::invalid("invalid pe header"));
    }
    let machine = u16::from_le_bytes(coff[4..6].try_into().unwrap());
    let sections = u16::from_le_bytes(coff[6..8].try_into().unwrap()) as usize;
    let optional_size = u16::from_le_bytes(coff[20..22].try_into().unwrap()) as usize;
    if sections > 4096 || optional_size > 4096 {
        return Err(Error::new(
            ErrorKind::LimitExceeded,
            "pe headers exceed bounds",
        ));
    }
    let mut optional = vec![0; optional_size];
    file.read_exact(&mut optional)?;
    let magic = optional
        .get(..2)
        .ok_or_else(|| Error::invalid("short pe optional header"))?;
    let directory = match u16::from_le_bytes(magic.try_into().unwrap()) {
        0x20b => 112,
        0x10b => 96,
        _ => {
            return Err(Error::new(
                ErrorKind::UnsupportedFormat,
                "unsupported pe optional format",
            ));
        }
    };
    let debug = optional
        .get(directory + 6 * 8..directory + 7 * 8)
        .ok_or_else(|| Error::invalid("pe has no debug directory"))?;
    let debug_rva = u32::from_le_bytes(debug[..4].try_into().unwrap());
    let debug_size = u32::from_le_bytes(debug[4..8].try_into().unwrap()) as usize;
    if debug_size == 0 || debug_size > 1024 * 1024 || !debug_size.is_multiple_of(28) {
        return Err(Error::new(
            ErrorKind::SymbolMismatch,
            "pe has no supported codeview directory",
        ));
    }
    let mut raw_offset = None;
    for _ in 0..sections {
        let mut section = [0u8; 40];
        file.read_exact(&mut section)?;
        let va = u32::from_le_bytes(section[12..16].try_into().unwrap());
        let size = u32::from_le_bytes(section[16..20].try_into().unwrap());
        let raw = u32::from_le_bytes(section[20..24].try_into().unwrap());
        if debug_rva >= va && debug_rva - va < size {
            raw_offset = Some(u64::from(raw) + u64::from(debug_rva - va));
        }
    }
    let raw =
        raw_offset.ok_or_else(|| Error::invalid("pe debug directory has no backing section"))?;
    file.seek(SeekFrom::Start(raw))?;
    let mut entries = vec![0; debug_size];
    file.read_exact(&mut entries)?;
    for entry in entries.chunks_exact(28) {
        if u32::from_le_bytes(entry[12..16].try_into().unwrap()) != 2 {
            continue;
        }
        let size = u32::from_le_bytes(entry[16..20].try_into().unwrap()) as usize;
        let offset = u32::from_le_bytes(entry[24..28].try_into().unwrap()) as u64;
        if !(24..=1024 * 1024).contains(&size) {
            return Err(Error::invalid("unsupported pe codeview size"));
        }
        file.seek(SeekFrom::Start(offset))?;
        let mut cv = [0; 24];
        file.read_exact(&mut cv)?;
        if &cv[..4] != b"RSDS" {
            return Err(Error::new(
                ErrorKind::UnsupportedFormat,
                "only RSDS pdb identities are implemented",
            ));
        }
        let a = u32::from_le_bytes(cv[4..8].try_into().unwrap());
        let b = u16::from_le_bytes(cv[8..10].try_into().unwrap());
        let c = u16::from_le_bytes(cv[10..12].try_into().unwrap());
        let age = u32::from_le_bytes(cv[20..24].try_into().unwrap());
        return Ok((
            format!(
                "{a:08X}-{b:04X}-{c:04X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}:{age}",
                cv[12], cv[13], cv[14], cv[15], cv[16], cv[17], cv[18], cv[19]
            ),
            machine,
        ));
    }
    Err(Error::new(
        ErrorKind::SymbolMismatch,
        "pe has no RSDS pdb identity",
    ))
}

// Mapping starts include the executable segment's leading page padding; sampled bytes do not.
pub(crate) fn elf_virtual_address(
    path: &Path,
    file_offset: u64,
    mapping_page_size: Option<u64>,
) -> Result<u64> {
    if let Some(page) = mapping_page_size
        && (!page.is_power_of_two() || !file_offset.is_multiple_of(page))
    {
        return Err(Error::invalid("invalid ELF mapping page alignment"));
    }
    let mut file = File::open(path)?;
    let mut header = [0u8; 64];
    file.read_exact(&mut header)?;
    if &header[..4] != b"\x7fELF" || header[4] != 2 || header[5] != 1 {
        return Err(Error::new(
            ErrorKind::UnsupportedFormat,
            "only 64-bit little-endian ELF addresses are implemented",
        ));
    }
    let phoff = u64::from_le_bytes(header[32..40].try_into().unwrap());
    let size = u16::from_le_bytes(header[54..56].try_into().unwrap()) as u64;
    let count = u16::from_le_bytes(header[56..58].try_into().unwrap()) as u64;
    if !(56..=4096).contains(&size) || count > 4096 {
        return Err(Error::invalid("invalid elf program headers"));
    }
    let mut result = None;
    for i in 0..count {
        file.seek(SeekFrom::Start(
            phoff
                .checked_add(i * size)
                .ok_or_else(|| Error::invalid("elf table overflow"))?,
        ))?;
        let mut entry = [0u8; 56];
        file.read_exact(&mut entry)?;
        if u32::from_le_bytes(entry[..4].try_into().unwrap()) != 1
            || u32::from_le_bytes(entry[4..8].try_into().unwrap()) & 1 == 0
        {
            continue;
        }
        let offset = u64::from_le_bytes(entry[8..16].try_into().unwrap());
        let virtual_address = u64::from_le_bytes(entry[16..24].try_into().unwrap());
        let length = u64::from_le_bytes(entry[32..40].try_into().unwrap());
        let end = offset
            .checked_add(length)
            .ok_or_else(|| Error::invalid("ELF executable segment range overflow"))?;
        let (start, virtual_start) = if let Some(page) = mapping_page_size {
            if offset % page != virtual_address % page {
                return Err(Error::invalid(
                    "ELF executable segment is not page-congruent",
                ));
            }
            (offset & !(page - 1), virtual_address & !(page - 1))
        } else {
            (offset, virtual_address)
        };
        if length != 0 && file_offset >= start && file_offset < end {
            let address = virtual_start
                .checked_add(file_offset - start)
                .ok_or_else(|| Error::invalid("ELF address overflow"))?;
            if result.is_some_and(|previous| previous != address) {
                return Err(Error::new(
                    ErrorKind::SymbolMismatch,
                    "ELF offset identifies conflicting executable segments",
                ));
            }
            result = Some(address);
        }
    }
    result.ok_or_else(|| {
        Error::new(
            ErrorKind::SymbolMismatch,
            "file offset has no matching executable ELF segment",
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn elf(
        executable_flags: u32,
        executable_virtual: u64,
        duplicate: bool,
    ) -> tempfile::NamedTempFile {
        use std::io::Write;
        let mut bytes = vec![0u8; 0x18000];
        bytes[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
        bytes[32..40].copy_from_slice(&64u64.to_le_bytes());
        bytes[54..56].copy_from_slice(&56u16.to_le_bytes());
        bytes[56..58].copy_from_slice(&(if duplicate { 3u16 } else { 2u16 }).to_le_bytes());
        for (index, (flags, offset, address, length)) in [
            (4u32, 0u64, 0u64, 0x13700u64),
            (executable_flags, 0x13700, executable_virtual, 0x2000),
            (5, 0x13700, 0x24700, 0x2000),
        ]
        .into_iter()
        .enumerate()
        {
            let entry = &mut bytes[64 + index * 56..64 + (index + 1) * 56];
            entry[..4].copy_from_slice(&1u32.to_le_bytes());
            entry[4..8].copy_from_slice(&flags.to_le_bytes());
            entry[8..16].copy_from_slice(&offset.to_le_bytes());
            entry[16..24].copy_from_slice(&address.to_le_bytes());
            entry[32..40].copy_from_slice(&length.to_le_bytes());
            entry[40..48].copy_from_slice(&length.to_le_bytes());
            entry[48..56].copy_from_slice(&4096u64.to_le_bytes());
        }
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(&bytes).unwrap();
        file
    }

    #[test]
    fn executable_mapping_padding_does_not_borrow_the_preceding_read_only_load() {
        let file = elf(5, 0x14700, false);
        assert_eq!(
            elf_virtual_address(file.path(), 0x13000, Some(4096)).unwrap(),
            0x14000
        );
        assert_eq!(
            elf_virtual_address(file.path(), 0x13a10, None).unwrap(),
            0x14a10
        );
        assert!(elf_virtual_address(file.path(), 0x13000, None).is_err());
        for offset in [0, 0x12000, 0x16000] {
            assert!(elf_virtual_address(file.path(), offset, Some(4096)).is_err());
        }
        assert!(elf_virtual_address(file.path(), 0x15700, None).is_err());
    }

    #[test]
    fn invalid_or_conflicting_executable_mapping_inputs_fail_explicitly() {
        let file = elf(5, 0x14700, false);
        for page in [0, 6000] {
            assert!(elf_virtual_address(file.path(), 0x13000, Some(page)).is_err());
        }
        assert!(elf_virtual_address(file.path(), 0x13700, Some(4096)).is_err());
        let nonexecuting = elf(4, 0x14700, false);
        assert!(elf_virtual_address(nonexecuting.path(), 0x13000, Some(4096)).is_err());
        let incongruent = elf(5, 0x14720, false);
        assert!(elf_virtual_address(incongruent.path(), 0x13000, Some(4096)).is_err());
        let ambiguous = elf(5, 0x14700, true);
        assert!(elf_virtual_address(ambiguous.path(), 0x13000, Some(4096)).is_err());
    }
}
