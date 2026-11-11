use hunin_platforms::{
    native::{Language, coverage::sha256},
    tools,
};
use serde_json::{Value, json};
use std::time::Duration;
use std::{
    path::{Path, PathBuf},
    process::Command,
};
pub fn executable(name: &str) -> PathBuf {
    if Path::new(name).is_absolute() {
        return PathBuf::from(name);
    }
    let filename = if cfg!(windows) && Path::new(name).extension().is_none() {
        format!("{name}.exe")
    } else {
        name.to_owned()
    };
    std::env::split_paths(&std::env::var_os("PATH").unwrap())
        .map(|path| path.join(&filename))
        .find(|path| path.is_file())
        .unwrap_or_else(|| panic!("missing selected executable:{name}"))
}
pub fn checked(command: &mut Command, temporary: &Path) -> String {
    tools::require_success(
        tools::run(command, temporary, Duration::from_secs(30), 1024 * 1024).unwrap(),
    )
    .unwrap()
    .stdout
}
#[cfg(target_os = "macos")]
pub fn coverage_tools(language: Language, temporary: &Path) -> (PathBuf, PathBuf) {
    if language == Language::Rust {
        let compiler = executable("rustc");
        let root = checked(
            Command::new(&compiler).args(["--print", "sysroot"]),
            temporary,
        );
        let verbose = checked(
            Command::new(compiler).args(["--version", "--verbose"]),
            temporary,
        );
        let host = verbose
            .lines()
            .find_map(|line| line.strip_prefix("host: "))
            .unwrap();
        let path = Path::new(root.trim())
            .join("lib/rustlib")
            .join(host)
            .join("bin");
        (path.join("llvm-profdata"), path.join("llvm-cov"))
    } else {
        let profdata = checked(
            Command::new("xcrun").args(["--find", "llvm-profdata"]),
            temporary,
        );
        let cov = checked(
            Command::new("xcrun").args(["--find", "llvm-cov"]),
            temporary,
        );
        (PathBuf::from(profdata.trim()), PathBuf::from(cov.trim()))
    }
}
pub fn compile(
    language: Language,
    temporary: &Path,
    instrument: bool,
) -> (PathBuf, String, Vec<String>) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    let binary = temporary.join(if cfg!(windows) {
        "preview.exe"
    } else {
        "preview"
    });
    let compiler = executable(if language == Language::Rust {
        "rustc"
    } else if cfg!(windows) {
        "cl.exe"
    } else if language == Language::Cpp {
        "clang++"
    } else {
        "clang"
    });
    let mut command = Command::new(&compiler);
    let version = if cfg!(windows) && language != Language::Rust {
        tools::run(
            Command::new(&compiler).arg("/Bv"),
            temporary,
            Duration::from_secs(10),
            65536,
        )
        .unwrap()
        .stderr
    } else {
        checked(Command::new(&compiler).arg("--version"), temporary)
    };
    if language == Language::Rust {
        command.args([
            "--edition=2024",
            "-g",
            "-C",
            "opt-level=1",
            "-C",
            "force-frame-pointers=yes",
        ]);
        if cfg!(target_os = "macos") && instrument {
            command.args(["-C", "instrument-coverage", "--cfg", "hunin_coverage"]);
        } else if cfg!(target_os = "linux") {
            command.args(["-C", "link-arg=-Wl,--build-id=sha1"]);
        }
        command.arg(root.join("preview.rs")).arg("-o").arg(&binary);
    } else if cfg!(windows) {
        command.args(["/nologo", "/Zi", "/Od", "/Oy-"]);
        command
            .arg(if language == Language::Cpp {
                "/TP"
            } else {
                "/TC"
            })
            .arg(root.join("preview.c"))
            .arg(format!("/Fo{}", temporary.join("preview.obj").display()))
            .arg(format!("/Fe{}", binary.display()))
            .args(["/link", "/DEBUG:FULL"])
            .arg(format!("/PDB:{}", temporary.join("preview.pdb").display()));
    } else {
        command.args(["-O1", "-g", "-fno-omit-frame-pointer", "-fno-inline"]);
        if language == Language::Cpp {
            command.args(["-x", "c++"]);
        }
        if cfg!(target_os = "macos") && instrument {
            command.args([
                "-fprofile-instr-generate",
                "-fcoverage-mapping",
                "-DHUNIN_COVERAGE",
            ]);
        } else if cfg!(target_os = "linux") {
            command.arg("-Wl,--build-id=sha1");
        }
        command.arg(root.join("preview.c")).arg("-o").arg(&binary);
    }
    let arguments = command
        .get_args()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect();
    checked(&mut command, temporary);
    (binary, version, arguments)
}
pub fn assets(directory: &Path) -> Vec<(String, usize, usize, Vec<u8>)> {
    std::fs::create_dir_all(directory).unwrap();
    [("alpha", 79, 41), ("beta", 48, 37), ("gamma", 96, 64)]
        .into_iter()
        .enumerate()
        .map(|(index, (stem, width, height))| {
            let pixels: Vec<_> = (0..height)
                .flat_map(|y| {
                    (0..width).flat_map(move |x| {
                        (0..3).map(move |c| ((x * 17 + y * 29 + c * 71 + index * 13) % 256) as u8)
                    })
                })
                .collect();
            let name = format!("{stem}.ppm");
            let mut bytes = format!("P6\n{width} {height}\n255\n").into_bytes();
            bytes.extend(&pixels);
            std::fs::write(directory.join(&name), bytes).unwrap();
            (name, width, height, pixels)
        })
        .collect()
}
pub fn check_outputs(directory: &Path, inputs: &[(String, usize, usize, Vec<u8>)]) -> Value {
    let manifest: Value =
        serde_json::from_slice(&std::fs::read(directory.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(manifest["algorithm"], "nearest-neighbor");
    assert_eq!(manifest["width"], 32);
    assert_eq!(manifest["height"], 32);
    let entries = manifest["assets"].as_array().unwrap();
    assert_eq!(entries.len(), inputs.len());
    let mut checks = Vec::new();
    for ((name, width, height, pixels), entry) in inputs.iter().zip(entries) {
        let stem = name.strip_suffix(".ppm").unwrap();
        let output = format!("{stem}.preview.ppm");
        let mut expected = b"P6\n32 32\n255\n".to_vec();
        let mut checksum = 0u64;
        for y in 0..32 {
            for x in 0..32 {
                let start = ((y * height / 32) * width + x * width / 32) * 3;
                for &channel in &pixels[start..start + 3] {
                    expected.push(channel);
                    checksum += u64::from(channel);
                }
            }
        }
        let path = directory.join(&output);
        assert_eq!(
            std::fs::read(&path).unwrap(),
            expected,
            "every generated PPM byte"
        );
        assert_eq!(entry["input"], name.as_str());
        assert_eq!(entry["output"], output);
        assert_eq!(entry["source_width"], *width);
        assert_eq!(entry["source_height"], *height);
        assert_eq!(entry["bytes"], 3085);
        assert_eq!(entry["checksum"], checksum);
        checks.push(json!({"input":name,"output":output,"sha256":sha256(&path).unwrap(),"bytes":expected.len(),"checksum":checksum}));
    }
    assert_eq!(
        std::fs::read_dir(directory).unwrap().count(),
        inputs.len() + 1
    );
    json!({"outcome":"passed","scope":"all manifest entries and every output byte","assets":checks})
}
