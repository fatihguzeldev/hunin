use std::{
    io::{BufRead, Write},
    path::Path,
    process::{Command, Stdio},
    sync::{Arc, Barrier, Mutex, mpsc},
    time::Instant,
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
fn thread_work(mut n: u64) -> u64 {
    for i in 0..50_000_000 {
        n = n
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407)
            ^ i;
    }
    std::hint::black_box(n)
}
#[inline(never)]
fn process_work(mut n: u64) -> u64 {
    for i in 0..50_000_000 {
        n = n
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407)
            ^ i;
    }
    std::hint::black_box(n)
}
#[inline(never)]
fn unrelated_work(mut n: u64) -> u64 {
    for i in 0..50_000_000 {
        n = n
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407)
            ^ i;
    }
    std::hint::black_box(n)
}
fn line(reader: &mut impl BufRead) -> String {
    let mut result = String::new();
    assert!(reader.read_line(&mut result).unwrap() > 0);
    result
}
fn emit(log: &Mutex<std::fs::File>, text: &str) {
    let mut file = log.lock().unwrap();
    writeln!(file, "{text}").unwrap();
    file.flush().unwrap();
}
fn event(
    id: &str,
    parent: &str,
    handoff: &str,
    token: &str,
    value: u64,
    start: u128,
    end: u128,
) -> String {
    format!(
        "{{\"id\":\"{id}\",\"parent\":{parent},\"handoff\":{handoff},\"token\":{token},\"pid\":{},\"value\":{value},\"start_ns\":{start},\"end_ns\":{end}}}",
        std::process::id()
    )
}
fn main() {
    let origin = Instant::now();
    let args: Vec<_> = std::env::args().collect();
    let stdin = std::io::stdin();
    let mut input = stdin.lock();
    if args.get(1).is_some_and(|arg| arg == "--child") {
        snapshot();
        println!("child-ready");
        std::io::stdout().flush().unwrap();
        assert_eq!(line(&mut input), "batch:process\n");
        let start = origin.elapsed().as_nanos();
        let value = process_work(3);
        let end = origin.elapsed().as_nanos();
        snapshot();
        println!(
            "{}",
            event(
                "process-work",
                "\"batch\"",
                "\"process\"",
                "\"batch:process\"",
                value,
                start,
                end
            )
        );
        println!("child-complete");
        std::io::stdout().flush().unwrap();
        assert_eq!(line(&mut input), "HUNIN_EXIT\n");
        std::process::exit(0);
    }
    let directory = Path::new(&args[1]);
    let log = Arc::new(Mutex::new(
        std::fs::File::create(directory.join("events.jsonl")).unwrap(),
    ));
    let mut child = Command::new(std::env::current_exe().unwrap())
        .arg("--child")
        .env("LLVM_PROFILE_FILE", directory.join("child.profraw"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut child_output = std::io::BufReader::new(child.stdout.take().unwrap());
    assert_eq!(line(&mut child_output), "child-ready\n");
    std::fs::write(directory.join("child.pid"), child.id().to_string()).unwrap();
    let barrier = Arc::new(Barrier::new(3));
    let (tx, rx) = mpsc::channel::<String>();
    let work_barrier = barrier.clone();
    let work_log = log.clone();
    let worker = std::thread::spawn(move || {
        let token = rx.recv().unwrap();
        assert_eq!(token, "batch:thread");
        work_barrier.wait();
        let start = origin.elapsed().as_nanos();
        let value = thread_work(1);
        let end = origin.elapsed().as_nanos();
        emit(
            &work_log,
            &event(
                "thread-work",
                "\"batch\"",
                "\"thread\"",
                "\"batch:thread\"",
                value,
                start,
                end,
            ),
        );
    });
    let noise_barrier = barrier.clone();
    let noise_log = log.clone();
    let unrelated = std::thread::spawn(move || {
        noise_barrier.wait();
        let start = origin.elapsed().as_nanos();
        let value = unrelated_work(2);
        let end = origin.elapsed().as_nanos();
        emit(
            &noise_log,
            &event("unrelated", "null", "null", "null", value, start, end),
        );
    });
    snapshot();
    println!("HUNIN_READY");
    std::io::stdout().flush().unwrap();
    assert_eq!(line(&mut input), "HUNIN_GO\n");
    let start = origin.elapsed().as_nanos();
    emit(
        &log,
        &event("batch", "null", "null", "null", 0, start, start),
    );
    tx.send("batch:thread".into()).unwrap();
    barrier.wait();
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(b"batch:process\n")
        .unwrap();
    worker.join().unwrap();
    unrelated.join().unwrap();
    emit(&log, line(&mut child_output).trim_end());
    assert_eq!(line(&mut child_output), "child-complete\n");
    snapshot();
    println!("HUNIN_WORK_COMPLETE");
    std::io::stdout().flush().unwrap();
    assert_eq!(line(&mut input), "HUNIN_EXIT\n");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"HUNIN_EXIT\n")
        .unwrap();
    assert!(child.wait().unwrap().success());
    std::process::exit(0);
}
