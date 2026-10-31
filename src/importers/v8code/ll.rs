use anyhow::{Context, Result, bail, ensure};

#[derive(Debug)]
pub(super) struct Bytes {
    pub address: u64,
    pub name: String,
    pub offset: u64,
    pub content: Vec<u8>,
}
pub(super) fn parse(raw: &[u8], architecture: &str) -> Result<Vec<Bytes>> {
    ensure!(
        matches!(architecture, "arm64" | "x64"),
        "unsupported V8 low-level ABI"
    );
    let header = raw
        .iter()
        .position(|b| *b == 0)
        .context("missing V8 low-level architecture")?;
    ensure!(
        &raw[..header] == architecture.as_bytes(),
        "V8 low-level architecture disagrees"
    );
    let mut at = header + 1;
    fn take<'a>(raw: &'a [u8], at: &mut usize, count: usize) -> Result<&'a [u8]> {
        let end = at
            .checked_add(count)
            .context("V8 low-level range overflows")?;
        let value = raw.get(*at..end).context("truncated V8 low-level record")?;
        *at = end;
        Ok(value)
    }
    let mut output = vec![];
    while at < raw.len() {
        match take(raw, &mut at, 1)?[0] {
            b'C' => {
                ensure!(output.len() < 100_000, "V8 low-level code limit exceeded");
                let header = take(raw, &mut at, 24)?;
                let names = i32::from_le_bytes(header[..4].try_into()?);
                let address = u64::from_le_bytes(header[8..16].try_into()?);
                let size = i32::from_le_bytes(header[16..20].try_into()?);
                ensure!(
                    (0..=1_048_576).contains(&names) && (1..=536_870_912).contains(&size),
                    "invalid V8 low-level record size"
                );
                let name = std::str::from_utf8(take(raw, &mut at, names as usize)?)?.to_owned();
                let offset = at as u64;
                let content = take(raw, &mut at, size as usize)?.to_vec();
                output.push(Bytes {
                    address,
                    name,
                    offset,
                    content,
                });
            }
            b'M' => {
                take(raw, &mut at, 16)?;
            }
            b'D' => {
                take(raw, &mut at, 8)?;
            }
            b'G' => {}
            tag => bail!("unsupported V8 low-level record tag {tag}"),
        }
    }
    Ok(output)
}
