use hunin_core::execution::PreparedProcess;
use hunin_runtimes::{RuntimeKind, prepare};
use std::{
    fs,
    path::PathBuf,
    time::{Duration, Instant},
};

#[test]
#[ignore = "requires all ten selected runtime/toolchain executables and HUNIN_XDEBUG"]
fn managed_control_is_not_application_stdout_and_survives_replaced_output_writers() {
    #[cfg(unix)]
    let root = tempfile::Builder::new()
        .prefix("hnc-")
        .tempdir_in("/tmp")
        .unwrap();
    #[cfg(not(unix))]
    let root = tempfile::Builder::new().prefix("hnc-").tempdir().unwrap();
    let expected = b"HUNIN_READY\nHUNIN_WORK_COMPLETE\npartial";
    for (name, kind, variable, filename, source) in [
        (
            "node",
            RuntimeKind::Node,
            "HUNIN_NODE",
            "control.cjs",
            javascript(),
        ),
        (
            "deno",
            RuntimeKind::Deno,
            "HUNIN_DENO",
            "control.cjs",
            javascript(),
        ),
        (
            "bun",
            RuntimeKind::Bun,
            "HUNIN_BUN",
            "control.cjs",
            javascript(),
        ),
        (
            "cpython",
            RuntimeKind::CPython,
            "HUNIN_PYTHON",
            "control.py",
            python(),
        ),
        (
            "pypy",
            RuntimeKind::PyPy,
            "HUNIN_PYPY",
            "control.py",
            python(),
        ),
        (
            "ruby",
            RuntimeKind::CRuby,
            "HUNIN_RUBY",
            "control.rb",
            ruby(),
        ),
        (
            "java",
            RuntimeKind::HotSpot,
            "HUNIN_JAVA",
            "ControlWorkload.java",
            java(),
        ),
        ("go", RuntimeKind::Go, "HUNIN_GO", "control.go", go()),
        ("php", RuntimeKind::PHP, "HUNIN_PHP", "control.php", php()),
        (
            "clr",
            RuntimeKind::CoreCLR,
            "HUNIN_DOTNET",
            "control.cs",
            clr(),
        ),
    ] {
        let executable = PathBuf::from(std::env::var_os(variable).expect(variable));
        let directory = root.path().join(name);
        let area = directory.join("capture");
        fs::create_dir(&directory).unwrap();
        fs::create_dir(&area).unwrap();
        let workload = directory.join(filename);
        fs::write(&workload, source).unwrap();
        let mut prepared = if kind == RuntimeKind::PHP {
            hunin_runtimes::php::prepare(
                &executable,
                &PathBuf::from(std::env::var_os("HUNIN_XDEBUG").unwrap()),
                &workload,
                &area,
            )
        } else {
            prepare(kind, &executable, &workload, &area)
        }
        .unwrap_or_else(|error| panic!("{name}: {error}"));
        assert_eq!(
            prepared.launch.environment["HUNIN_CONTROL_PROTOCOL"],
            "nonce-lines-v1"
        );
        assert!(
            !prepared
                .launch
                .environment
                .contains_key("HUNIN_CONTROL_TOKEN")
        );
        prepared
            .launch
            .environment
            .insert("HUNIN_FINALIZE_GATE".into(), "1".into());
        let mut process = PreparedProcess::spawn(&prepared.launch).unwrap();
        process.await_ready(Duration::from_secs(15)).unwrap();
        assert!(!prepared.report_path.exists());
        process.release().unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        while !process.work_complete().unwrap() {
            assert!(Instant::now() < deadline, "{name}: completion deadline");
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(
            prepared.report_path.exists(),
            "{name}: accepted application text before the helper exported its outcome"
        );
        let report: serde_json::Value =
            serde_json::from_slice(&fs::read(&prepared.report_path).unwrap()).unwrap();
        assert_eq!(report["outcome"], "completed", "{name}: {report}");
        assert_eq!(report["output"], "done", "{name}: {report}");
        process.finish().unwrap();
        let outcome = process.wait(Duration::from_secs(10)).unwrap();
        assert!(outcome.success, "{name}: {outcome:?}");
        assert!(
            outcome.measured_elapsed.unwrap() >= Duration::from_millis(120),
            "{name}: premature completion {outcome:?}"
        );
        assert_eq!(
            outcome.stdout, expected,
            "{name}: application bytes changed"
        );
        eprintln!("{name}: nonce completion followed actual work; stdout preserved");

        let legacy_area = directory.join("legacy");
        fs::create_dir(&legacy_area).unwrap();
        fs::write(&workload, legacy_workload(kind)).unwrap();
        let mut legacy = if kind == RuntimeKind::PHP {
            hunin_runtimes::php::prepare(
                &executable,
                &PathBuf::from(std::env::var_os("HUNIN_XDEBUG").unwrap()),
                &workload,
                &legacy_area,
            )
        } else {
            prepare(kind, &executable, &workload, &legacy_area)
        }
        .unwrap();
        legacy.launch.environment.remove("HUNIN_CONTROL_PROTOCOL");
        legacy
            .launch
            .environment
            .insert("HUNIN_FINALIZE_GATE".into(), "1".into());
        let mut process = PreparedProcess::spawn(&legacy.launch).unwrap();
        process.await_ready(Duration::from_secs(15)).unwrap();
        process.release().unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        while !process.work_complete().unwrap() {
            assert!(
                Instant::now() < deadline,
                "{name}: legacy completion deadline"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(legacy.report_path.exists());
        process.finish().unwrap();
        let outcome = process.wait(Duration::from_secs(10)).unwrap();
        assert!(outcome.success, "{name}: legacy {outcome:?}");
        assert!(
            outcome.stdout == b"HUNIN_WORK_COMPLETE\n"
                || outcome.stdout == b"HUNIN_WORK_COMPLETE\r\n",
            "{name}: legacy stdout convention changed: {outcome:?}"
        );
        eprintln!("{name}: trusted legacy plain control remains usable without token");
    }
}

fn legacy_workload(kind: RuntimeKind) -> &'static str {
    match kind {
        RuntimeKind::Node | RuntimeKind::Deno | RuntimeKind::Bun => "exports.run=()=> 'done';",
        RuntimeKind::CPython | RuntimeKind::PyPy => "def run(): return 'done'\n",
        RuntimeKind::CRuby => "module HuninWorkload;def self.run;'done';end;end\n",
        RuntimeKind::HotSpot => {
            "public class ControlWorkload {public static Object run(){return \"done\";}}\n"
        }
        RuntimeKind::Go => "package main\nfunc run()any{return \"done\"}\n",
        RuntimeKind::PHP => "<?php function run(){return 'done';}\n",
        RuntimeKind::CoreCLR => {
            "public static class ControlWorkload{public static object Run(){return \"done\";}}\n"
        }
    }
}

fn javascript() -> &'static str {
    r#"exports.run=()=>{process.stdout.write('HUNIN_READY\nHUNIN_WORK_COMPLETE\npartial');process.env.HUNIN_CONTROL_TOKEN='wrong';process.env.HUNIN_FINALIZE_GATE='0';process.stdout.write=()=>true;const until=performance.now()+150;while(performance.now()<until){};return 'done';};"#
}
fn python() -> &'static str {
    "import sys,time,os,io\ndef run():\n sys.stdout.write('HUNIN_READY\\nHUNIN_WORK_COMPLETE\\npartial');sys.stdout.flush()\n os.environ['HUNIN_CONTROL_TOKEN']='wrong';os.environ['HUNIN_FINALIZE_GATE']='0';sys.stdout=io.StringIO()\n until=time.monotonic()+0.15\n while time.monotonic()<until: pass\n return 'done'\n"
}
fn ruby() -> &'static str {
    "require 'stringio'\nmodule HuninWorkload\n def self.run\n  STDOUT.write(\"HUNIN_READY\\nHUNIN_WORK_COMPLETE\\npartial\");STDOUT.flush\n  ENV['HUNIN_CONTROL_TOKEN']='wrong';ENV['HUNIN_FINALIZE_GATE']='0'\n  $stdout=StringIO.new;Object.send(:remove_const,:STDOUT);Object.const_set(:STDOUT,$stdout)\n  until_ns=Process.clock_gettime(Process::CLOCK_MONOTONIC,:nanosecond)+150000000\n  while Process.clock_gettime(Process::CLOCK_MONOTONIC,:nanosecond)<until_ns;end\n  'done'\n end\nend\n"
}
fn java() -> &'static str {
    r#"public class ControlWorkload {public static Object run(){System.out.print("HUNIN_READY\nHUNIN_WORK_COMPLETE\npartial");System.out.flush();System.setOut(new java.io.PrintStream(java.io.OutputStream.nullOutputStream()));long until=System.nanoTime()+150000000L;while(System.nanoTime()<until){}return "done";}}"#
}
fn go() -> &'static str {
    "package main\nimport (\"fmt\";\"os\";\"time\")\nfunc run() any {fmt.Print(\"HUNIN_READY\\nHUNIN_WORK_COMPLETE\\npartial\");os.Setenv(\"HUNIN_CONTROL_TOKEN\",\"wrong\");os.Setenv(\"HUNIN_FINALIZE_GATE\",\"0\");os.Stdout=nil;until:=time.Now().Add(150*time.Millisecond);for time.Now().Before(until){};return \"done\"}\n"
}
fn php() -> &'static str {
    r#"<?php function run(){fwrite(STDOUT,"HUNIN_READY\nHUNIN_WORK_COMPLETE\npartial");fflush(STDOUT);putenv('HUNIN_CONTROL_TOKEN=wrong');putenv('HUNIN_FINALIZE_GATE=0');ob_start();$until=hrtime(true)+150000000;while(hrtime(true)<$until){}return 'done';}"#
}
fn clr() -> &'static str {
    r#"public static class ControlWorkload {public static object Run(){System.Console.Write("HUNIN_READY\nHUNIN_WORK_COMPLETE\npartial");System.Console.Out.Flush();System.Environment.SetEnvironmentVariable("HUNIN_CONTROL_TOKEN","wrong");System.Environment.SetEnvironmentVariable("HUNIN_FINALIZE_GATE","0");System.Console.SetOut(System.IO.TextWriter.Null);var watch=System.Diagnostics.Stopwatch.StartNew();while(watch.ElapsedMilliseconds<150){}return "done";}}"#
}
