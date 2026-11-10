use crate::process::{Error, ErrorKind, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileEvent {
    pub instruction_pointer: u64,
    pub thread_id: u32,
    pub count: u16,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StackEvent {
    pub timestamp: u64,
    pub process_id: u32,
    pub thread_id: u32,
    pub addresses: Vec<u64>,
}
fn word(bytes: &[u8], offset: usize, width: usize) -> Result<u64> {
    let end = offset
        .checked_add(width)
        .ok_or_else(|| Error::invalid("etw field overflow"))?;
    let b = bytes
        .get(offset..end)
        .ok_or_else(|| Error::invalid("truncated etw field"))?;
    match width {
        4 => Ok(u32::from_le_bytes(b.try_into().unwrap()) as u64),
        8 => Ok(u64::from_le_bytes(b.try_into().unwrap())),
        _ => Err(Error::invalid("etw pointer width must be 4 or 8")),
    }
}
pub fn profile_event(bytes: &[u8], pointer_width: usize, version: u8) -> Result<ProfileEvent> {
    if version > 2 || bytes.len() != pointer_width + 8 {
        return Err(Error::new(
            ErrorKind::UnsupportedFormat,
            "unsupported sampled-profile event version or layout",
        ));
    }
    Ok(ProfileEvent {
        instruction_pointer: word(bytes, 0, pointer_width)?,
        thread_id: word(bytes, pointer_width, 4)? as u32,
        count: u16::from_le_bytes(
            bytes[pointer_width + 4..pointer_width + 6]
                .try_into()
                .unwrap(),
        ),
    })
}
pub fn stack_event(
    bytes: &[u8],
    pointer_width: usize,
    version: u8,
    maximum_frames: usize,
) -> Result<StackEvent> {
    if version != 2
        || ![4, 8].contains(&pointer_width)
        || bytes.len() < 16
        || !(bytes.len() - 16).is_multiple_of(pointer_width)
    {
        return Err(Error::new(
            ErrorKind::UnsupportedFormat,
            "unsupported stackwalk event version or layout",
        ));
    }
    let count = (bytes.len() - 16) / pointer_width;
    if count > maximum_frames {
        return Err(Error::new(
            ErrorKind::LimitExceeded,
            "etw stack exceeds frame budget",
        ));
    }
    Ok(StackEvent {
        timestamp: word(bytes, 0, 8)?,
        process_id: word(bytes, 8, 4)? as u32,
        thread_id: word(bytes, 12, 4)? as u32,
        addresses: (0..count)
            .map(|i| word(bytes, 16 + i * pointer_width, pointer_width))
            .collect::<Result<_>>()?,
    })
}
