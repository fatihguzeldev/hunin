use std::{
    fs,
    io::{BufRead, Write},
    path::Path,
};
#[cfg(hunin_coverage)]
unsafe extern "C" {
    fn __llvm_profile_write_file() -> i32;
}
fn snapshot() {
    #[cfg(hunin_coverage)]
    assert_eq!(unsafe { __llvm_profile_write_file() }, 0);
}
#[inline(never)]
fn resize_preview(input: &[u8], width: usize, height: usize, output: &mut [u8]) -> u64 {
    let mut checksum = 0;
    for y in 0..32 {
        for x in 0..32 {
            for c in 0..3 {
                let value = input[((y * height / 32) * width + x * width / 32) * 3 + c];
                output[(y * 32 + x) * 3 + c] = value;
                checksum += u64::from(value);
            }
        }
    }
    checksum
}
fn word<'a>(bytes: &'a [u8], cursor: &mut usize) -> &'a [u8] {
    while bytes[*cursor].is_ascii_whitespace() {
        *cursor += 1;
    }
    let start = *cursor;
    while !bytes[*cursor].is_ascii_whitespace() {
        *cursor += 1;
    }
    &bytes[start..*cursor]
}
fn main() {
    let arguments: Vec<_> = std::env::args().collect();
    assert_eq!(arguments.len(), 4);
    let input = Path::new(&arguments[1]);
    let output = Path::new(&arguments[2]);
    let passes = if arguments[3] == "redundant" { 2 } else { 1 };
    let repetitions: usize = std::env::var("PREVIEW_REPETITIONS")
        .unwrap_or_else(|_| "10000".into())
        .parse()
        .unwrap();
    assert!((1..=1000000).contains(&repetitions));
    let mut files: Vec<_> = fs::read_dir(input)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "ppm"))
        .collect();
    files.sort();
    assert!(!files.is_empty());
    snapshot();
    println!("HUNIN_READY");
    std::io::stdout().flush().unwrap();
    let stdin = std::io::stdin();
    let mut reader = stdin.lock();
    let mut gate = String::new();
    reader.read_line(&mut gate).unwrap();
    if gate != "HUNIN_GO\n" {
        std::process::exit(0);
    }
    let work_start = std::time::Instant::now();
    let mut manifest =
        String::from("{\"algorithm\":\"nearest-neighbor\",\"width\":32,\"height\":32,\"assets\":[");
    for (index, path) in files.iter().enumerate() {
        let bytes = fs::read(path).unwrap();
        let mut cursor = 0;
        assert_eq!(word(&bytes, &mut cursor), b"P6");
        let width: usize = std::str::from_utf8(word(&bytes, &mut cursor))
            .unwrap()
            .parse()
            .unwrap();
        let height: usize = std::str::from_utf8(word(&bytes, &mut cursor))
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(word(&bytes, &mut cursor), b"255");
        cursor += 1;
        let pixels = &bytes[cursor..];
        assert_eq!(pixels.len(), width * height * 3);
        let mut preview = vec![0u8; 3072];
        let mut checksum = 0;
        for _ in 0..passes {
            for _ in 0..repetitions {
                checksum = resize_preview(
                    std::hint::black_box(pixels),
                    width,
                    height,
                    std::hint::black_box(&mut preview),
                );
                std::hint::black_box(checksum);
            }
        }
        let filename = path.file_name().unwrap().to_str().unwrap();
        let name = format!(
            "{}.preview.ppm",
            path.file_stem().unwrap().to_str().unwrap()
        );
        let mut result = b"P6\n32 32\n255\n".to_vec();
        result.extend(&preview);
        fs::write(output.join(&name), result).unwrap();
        if index > 0 {
            manifest.push(',');
        }
        manifest.push_str(&format!("{{\"input\":\"{filename}\",\"output\":\"{name}\",\"source_width\":{width},\"source_height\":{height},\"bytes\":3085,\"checksum\":{checksum}}}"));
    }
    manifest.push_str("]}");
    fs::write(output.join("manifest.json"), manifest).unwrap();
    let work_elapsed = work_start.elapsed().as_nanos();
    if let Ok(path) = std::env::var("HUNIN_WORK_METRICS") {
        fs::write(path,format!("{{\"start\":0,\"end\":{work_elapsed},\"ticks_per_second\":1000000000,\"scope\":\"asset decode/resize/output batch excluding profile snapshots; input names prepared before ready\"}}")).unwrap();
    }
    snapshot();
    println!("HUNIN_WORK_COMPLETE");
    std::io::stdout().flush().unwrap();
    gate.clear();
    reader.read_line(&mut gate).unwrap();
    assert_eq!(gate, "HUNIN_EXIT\n");
    std::process::exit(0);
}
