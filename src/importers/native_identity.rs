use anyhow::{Context, Result};
use object::{
    Object,
    read::macho::FatArch,
    read::pe::{ImageNtHeaders, ImageOptionalHeader},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fs, io::Read, path::Path};

const MAX_IMAGE: u64 = 128 * 1024 * 1024;
const MAX_TOTAL: u64 = 256 * 1024 * 1024;

#[derive(Default)]
pub(super) struct Images {
    checks: BTreeMap<String, Value>,
}
#[derive(Debug)]
struct Identifiers {
    code: Option<String>,
    debug: Option<String>,
    format: &'static str,
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02X}")).collect()
}
fn normalized(id: &str) -> String {
    id.replace('-', "").to_ascii_uppercase()
}
fn thin(bytes: &[u8]) -> Result<Identifiers> {
    let image = object::File::parse(bytes)?;
    if let Some(uuid) = image.mach_uuid()? {
        let code = hex(&uuid);
        return Ok(Identifiers {
            debug: Some(format!("{code}0")),
            code: Some(code),
            format: "Mach-O UUID",
        });
    }
    if let Some(build) = image.build_id()? {
        return Ok(Identifiers {
            code: Some(hex(build)),
            debug: None,
            format: "ELF GNU build ID",
        });
    }
    let pe_code = match &image {
        object::File::Pe32(pe) => Some(format!(
            "{:08X}{:X}",
            pe.nt_headers()
                .file_header()
                .time_date_stamp
                .get(object::LittleEndian),
            pe.nt_headers().optional_header().size_of_image()
        )),
        object::File::Pe64(pe) => Some(format!(
            "{:08X}{:X}",
            pe.nt_headers()
                .file_header()
                .time_date_stamp
                .get(object::LittleEndian),
            pe.nt_headers().optional_header().size_of_image()
        )),
        _ => None,
    };
    if pe_code.is_some() {
        let debug = image.pdb_info()?.map(|pdb| {
            let guid = pdb.guid();
            format!(
                "{:08X}{:04X}{:04X}{}{:X}",
                u32::from_le_bytes(guid[..4].try_into().unwrap()),
                u16::from_le_bytes(guid[4..6].try_into().unwrap()),
                u16::from_le_bytes(guid[6..8].try_into().unwrap()),
                hex(&guid[8..]),
                pdb.age()
            )
        });
        return Ok(Identifiers {
            code: pe_code,
            debug,
            format: "PE timestamp/image size and CodeView GUID/age",
        });
    }
    Ok(Identifiers {
        code: None,
        debug: None,
        format: "unavailable build identifier",
    })
}
fn identifiers(bytes: &[u8]) -> Result<Vec<Identifiers>> {
    Ok(match object::FileKind::parse(bytes)? {
        object::FileKind::MachOFat32 => object::read::macho::MachOFatFile32::parse(bytes)?
            .arches()
            .iter()
            .map(|arch| thin(arch.data(bytes)?))
            .collect::<Result<_>>()?,
        object::FileKind::MachOFat64 => object::read::macho::MachOFatFile64::parse(bytes)?
            .arches()
            .iter()
            .map(|arch| thin(arch.data(bytes)?))
            .collect::<Result<_>>()?,
        _ => vec![thin(bytes)?],
    })
}
fn check(lib: &Value, total: &mut u64) -> Value {
    let mut value = json!({"module":lib["name"],"path":lib["path"],"reported_code_id":lib["codeId"],"reported_debug_id":lib["breakpadId"],"status":"unverified","scope":"Local image identifiers checked at normalization; load instance/lifetime and source-content build relation remain unverified"});
    let result = (|| -> Result<()> {
        let path = Path::new(lib["path"].as_str().context("profile has no image path")?);
        let mut file = fs::File::open(path).context("image is unavailable on this host")?;
        let size = file.metadata()?.len();
        anyhow::ensure!(
            size <= MAX_IMAGE && total.saturating_add(size) <= MAX_TOTAL,
            "image identity verification exceeds bounded read budget"
        );
        let mut header = [0; 64];
        let count = file.read(&mut header)?;
        if !header[..count].starts_with(b"MZ") {
            object::FileKind::parse(&header[..count])
                .context("profile path is not a supported native image")?;
        }
        let mut bytes = Vec::new();
        use std::io::{Seek, SeekFrom};
        file.seek(SeekFrom::Start(0))?;
        file.take(MAX_IMAGE + 1).read_to_end(&mut bytes)?;
        anyhow::ensure!(
            bytes.len() as u64 <= MAX_IMAGE
                && total.saturating_add(bytes.len() as u64) <= MAX_TOTAL,
            "image grew beyond the bounded read budget"
        );
        *total += bytes.len() as u64;
        value["image_sha256"] = json!(format!("{:x}", Sha256::digest(&bytes)));
        let ids = identifiers(&bytes)?;
        value["local_identifiers"] = json!(
            ids.iter()
                .map(|id| json!({"code_id":id.code,"debug_id":id.debug,"format":id.format}))
                .collect::<Vec<_>>()
        );
        let expected_code = lib["codeId"]
            .as_str()
            .filter(|v| !v.is_empty())
            .map(normalized);
        let expected_debug = lib["breakpadId"]
            .as_str()
            .filter(|v| !v.is_empty())
            .map(normalized);
        let mut comparable = false;
        let matched = ids.iter().any(|id| {
            let code = expected_code.as_ref().zip(id.code.as_ref()).map(|(a, b)| {
                comparable = true;
                a == &normalized(b)
            });
            let debug = expected_debug
                .as_ref()
                .zip(id.debug.as_ref())
                .map(|(a, b)| {
                    comparable = true;
                    a == &normalized(b)
                });
            (code.is_some() || debug.is_some()) && code != Some(false) && debug != Some(false)
        });
        value["status"] = json!(if matched {
            "identifier_match"
        } else if comparable {
            "mismatch"
        } else {
            "unverified"
        });
        if !comparable {
            value["diagnostic"] = json!("no comparable image build identifiers");
        }
        Ok(())
    })();
    if let Err(error) = result {
        value["diagnostic"] = json!(format!("{error:#}"));
    }
    value
}
impl Images {
    pub fn inspect(raw: &Value) -> Self {
        fn walk(raw: &Value, images: &mut Images, total: &mut u64) {
            for lib in raw["libs"].as_array().into_iter().flatten() {
                let key = lib.to_string();
                images
                    .checks
                    .entry(key)
                    .or_insert_with(|| check(lib, total));
            }
            for child in raw["processes"].as_array().into_iter().flatten() {
                walk(child, images, total);
            }
        }
        let mut images = Self::default();
        walk(raw, &mut images, &mut 0);
        images
    }
    pub fn get(&self, lib: &Value) -> Option<&Value> {
        self.checks.get(&lib.to_string())
    }
    pub fn fingerprint(&self) -> Value {
        json!(self.checks)
    }
    pub fn diagnostics(&self) -> Vec<String> {
        let mut result: Vec<_> = self.checks.values().filter(|c| c["status"] == "mismatch").map(|c| format!("Native image identity mismatch for {} ({}): profile code/debug identifiers disagree with the local image; function and source positions remain unresolved", c["module"].as_str().unwrap_or("[unknown]"), c["path"].as_str().unwrap_or("[unknown]"))).collect();
        let unknown = self
            .checks
            .values()
            .filter(|c| c["status"] == "unverified")
            .count();
        if unknown > 0 {
            result.push(format!("{unknown} native image identities could not be checked against local image bytes; profile/sidecar symbols remain unverified and do not establish loaded-image lifetime or source-build identity"));
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn u16_at(v: &mut [u8], at: usize, value: u16) {
        v[at..at + 2].copy_from_slice(&value.to_le_bytes());
    }
    fn u32_at(v: &mut [u8], at: usize, value: u32) {
        v[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }
    fn u64_at(v: &mut [u8], at: usize, value: u64) {
        v[at..at + 8].copy_from_slice(&value.to_le_bytes());
    }
    fn elf() -> Vec<u8> {
        let mut v = vec![0; 156];
        v[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
        u16_at(&mut v, 16, 3);
        u16_at(&mut v, 18, 62);
        u32_at(&mut v, 20, 1);
        u64_at(&mut v, 32, 64);
        u16_at(&mut v, 52, 64);
        u16_at(&mut v, 54, 56);
        u16_at(&mut v, 56, 1);
        u32_at(&mut v, 64, 4);
        u32_at(&mut v, 68, 4);
        u64_at(&mut v, 72, 120);
        u64_at(&mut v, 96, 36);
        u64_at(&mut v, 104, 36);
        u64_at(&mut v, 112, 4);
        u32_at(&mut v, 120, 4);
        u32_at(&mut v, 124, 20);
        u32_at(&mut v, 128, 3);
        v[132..136].copy_from_slice(b"GNU\0");
        for i in 0..20 {
            v[136 + i] = i as u8 + 1;
        }
        v
    }
    fn pe() -> Vec<u8> {
        let mut v = vec![0; 1024];
        v[..2].copy_from_slice(b"MZ");
        u32_at(&mut v, 0x3c, 0x80);
        v[0x80..0x84].copy_from_slice(b"PE\0\0");
        u16_at(&mut v, 0x84, 0x8664);
        u16_at(&mut v, 0x86, 1);
        u32_at(&mut v, 0x88, 0x12345678);
        u16_at(&mut v, 0x94, 240);
        u16_at(&mut v, 0x96, 0x2022);
        u16_at(&mut v, 0x98, 0x20b);
        u32_at(&mut v, 0x98 + 56, 0x2000);
        u32_at(&mut v, 0x98 + 60, 0x200);
        u32_at(&mut v, 0x98 + 108, 16);
        u32_at(&mut v, 0x98 + 112 + 6 * 8, 0x1000);
        u32_at(&mut v, 0x98 + 112 + 6 * 8 + 4, 28);
        v[0x188..0x18e].copy_from_slice(b".rdata");
        u32_at(&mut v, 0x188 + 8, 0x200);
        u32_at(&mut v, 0x188 + 12, 0x1000);
        u32_at(&mut v, 0x188 + 16, 0x200);
        u32_at(&mut v, 0x188 + 20, 0x200);
        u32_at(&mut v, 0x188 + 36, 0x40000040);
        u32_at(&mut v, 0x200 + 12, 2);
        u32_at(&mut v, 0x200 + 16, 30);
        u32_at(&mut v, 0x200 + 20, 0x1020);
        u32_at(&mut v, 0x200 + 24, 0x220);
        v[0x220..0x224].copy_from_slice(b"RSDS");
        for i in 0..16 {
            v[0x224 + i] = i as u8;
        }
        u32_at(&mut v, 0x234, 2);
        v[0x238..0x23e].copy_from_slice(b"a.pdb\0");
        v
    }
    #[test]
    fn actual_mach_images_have_distinct_identifiers() {
        let a = identifiers(include_bytes!("fixtures/native-reload/phase_a.macho")).unwrap();
        let b = identifiers(include_bytes!("fixtures/native-reload/phase_b.macho")).unwrap();
        assert_eq!(
            a[0].code.as_deref(),
            Some("CD3C90FFC4B93EF1B5FD3FBD731EA331")
        );
        assert_eq!(
            b[0].code.as_deref(),
            Some("9704B91E012B334DA0662C41D7514208")
        );
    }
    #[test]
    fn elf_build_id_and_pe_codeview_byte_order_are_checked() {
        let elf = identifiers(&elf()).unwrap();
        assert_eq!(
            elf[0].code.as_deref(),
            Some("0102030405060708090A0B0C0D0E0F1011121314")
        );
        let pe = pe();
        let ids = identifiers(&pe).unwrap();
        assert_eq!(ids[0].code.as_deref(), Some("123456782000"));
        assert_eq!(
            ids[0].debug.as_deref(),
            Some("030201000504070608090A0B0C0D0E0F2")
        );
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("image.exe");
        fs::write(&path, &pe).unwrap();
        let mut lib = json!({"name":"image.exe","path":path,"codeId":"123456782000","breakpadId":"030201000504070608090A0B0C0D0E0F2"});
        assert_eq!(check(&lib, &mut 0)["status"], "identifier_match");
        lib["breakpadId"] = json!("030201000504070608090A0B0C0D0E0F3");
        assert_eq!(check(&lib, &mut 0)["status"], "mismatch");
    }
    #[test]
    fn missing_images_and_missing_identifiers_do_not_claim_verified_mapping() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("image.elf");
        fs::write(&path, elf()).unwrap();
        let lib = json!({"name":"image.elf","path":path});
        assert_eq!(check(&lib, &mut 0)["status"], "unverified");
        fs::remove_file(&path).unwrap();
        assert_eq!(check(&lib, &mut 0)["status"], "unverified");
    }
}
