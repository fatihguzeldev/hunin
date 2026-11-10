use hunin_app::{BeginCapture, CapturePlan};
use hunin_core::{control::RequestId, project::Project};
use hunin_runtimes::RuntimeKind;
use serde_json::{Value, json};
use std::{collections::BTreeMap, fs, path::PathBuf, time::Duration};

fn nanoseconds(value: &Value) -> u64 {
    value
        .as_u64()
        .unwrap_or_else(|| value.as_str().unwrap().parse().unwrap())
}

#[test]
#[ignore = "requires selected HUNIN_NODE and HUNIN_PYTHON; exercises real process CPU and application imports"]
fn application_stdout_cannot_arm_or_complete_node_and_python_captures() {
    let area = tempfile::tempdir().unwrap();
    for (name, kind, variable, extension) in [
        ("node", RuntimeKind::Node, "HUNIN_NODE", "cjs"),
        ("python", RuntimeKind::CPython, "HUNIN_PYTHON", "py"),
    ] {
        let executable = PathBuf::from(std::env::var_os(variable).expect("set selected runtime"));
        for mode in [
            "literal-completion",
            "ordinary-import-log",
            "partial-stdout-prefix",
            "plain-control",
        ] {
            let directory = area.path().join(format!("{name}-{mode}"));
            fs::create_dir(&directory).unwrap();
            let workload = directory.join(format!("work.{extension}"));
            let completion = mode == "literal-completion";
            let import_log = mode == "ordinary-import-log";
            let partial = mode == "partial-stdout-prefix";
            let source = if kind == RuntimeKind::Node {
                format!(
                    "{}exports.run=()=>{{{}const start=process.cpuUsage();let iterations=0;for(;;){{for(let i=0;i<10000;i++)iterations+=i;const cpu=process.cpuUsage(start);if(cpu.user+cpu.system>=150000)return {{cpu_ns:String((cpu.user+cpu.system)*1000),iterations}};}}}};\n",
                    if import_log {
                        "console.log('ordinary import log');console.log('HUNIN_READY');"
                    } else if partial {
                        "process.stdout.write('partial import ');"
                    } else {
                        ""
                    },
                    if completion {
                        "console.log('HUNIN_WORK_COMPLETE');"
                    } else if partial {
                        "process.stdout.write('partial work ');"
                    } else {
                        ""
                    },
                )
            } else {
                format!(
                    "import sys,time\n{}def run():\n    {}start=time.process_time_ns()\n    iterations=0\n    while time.process_time_ns()-start < 150000000:\n        iterations+=1\n    return {{'cpu_ns':str(time.process_time_ns()-start),'iterations':iterations}}\n",
                    if import_log {
                        "print('ordinary import log',flush=True)\nprint('HUNIN_READY',flush=True)\n"
                    } else if partial {
                        "sys.stdout.write('partial import ');sys.stdout.flush()\n"
                    } else {
                        ""
                    },
                    if completion {
                        "print('HUNIN_WORK_COMPLETE',flush=True)\n    "
                    } else if partial {
                        "sys.stdout.write('partial work ');sys.stdout.flush()\n    "
                    } else {
                        ""
                    },
                )
            };
            fs::write(&workload, source).unwrap();
            let project = Project::create(&directory, "p").unwrap();
            let investigation = project
                .create_investigation("i", "stdout control boundary", json!({}))
                .unwrap();
            let run = project
                .create_run(&investigation, "r", json!({"mode":mode}))
                .unwrap();
            let plan = CapturePlan {
                request: RequestId {
                    project: "p".into(),
                    caller: "test".into(),
                    id: "capture".into(),
                },
                run,
                runtime: kind,
                executable: executable.clone(),
                workload,
                conditions: json!({"expected_cpu_ns":150_000_000u64}),
                environment: BTreeMap::new(),
                read_paths: vec![],
                write_paths: vec![],
                xdebug_extension: None,
            };
            let BeginCapture::Started(mut capture) = hunin_app::begin(project, plan).unwrap()
            else {
                panic!("fresh capture")
            };
            capture.release().unwrap();
            let report = capture.finish(Duration::from_secs(10)).unwrap();
            assert!(
                report.application_success,
                "{name}/{mode}: {}",
                report.application
            );
            assert_eq!(report.request.state, "succeeded");
            assert_eq!(report.application["outcome"], "completed");
            let work_cpu = nanoseconds(&report.application["output"]["cpu_ns"]);
            assert!(work_cpu >= 150_000_000);
            assert!(nanoseconds(&report.application["output"]["iterations"]) > 0);
            let (start, end) = if kind == RuntimeKind::Node {
                (
                    &report.application["workStartNs"],
                    &report.application["workEndNs"],
                )
            } else {
                (
                    &report.application["work_start_ns"],
                    &report.application["work_end_ns"],
                )
            };
            let helper_elapsed = nanoseconds(end).checked_sub(nanoseconds(start)).unwrap();
            let controller_elapsed = report.elapsed_ns.parse::<u64>().unwrap();
            let collected_cpu = report.cpu_ns.parse::<u64>().unwrap();
            assert!(
                controller_elapsed + 2_000_000 >= helper_elapsed,
                "{name}/{mode}: ordinary stdout ended the controller window early: controller={controller_elapsed}ns, helper={helper_elapsed}ns"
            );
            assert!(
                collected_cpu + 20_000_000 >= work_cpu,
                "{name}/{mode}: resource window missed known work: collected={collected_cpu}ns, target={work_cpu}ns"
            );
            assert!(
                report.resource_observations >= 3,
                "{name}/{mode}: missing in-work acquisitions"
            );
            eprintln!(
                "{}",
                json!({"family":name,"mode":mode,"controller_elapsed_ns":controller_elapsed,"helper_elapsed_ns":helper_elapsed,"collected_cpu_ns":collected_cpu,"target_cpu_ns":work_cpu,"resource_observations":report.resource_observations})
            );
        }
    }
}

#[test]
#[ignore = "requires selected HUNIN_NODE and headless HUNIN_CHROME; never starts a GUI browser"]
fn browser_driver_stdout_cannot_rewrite_or_complete_the_control_channel() {
    let area = tempfile::tempdir().unwrap();
    let workload = area.path().join("work.html");
    fs::write(&workload,"<!doctype html><script>globalThis.run=function controlWork(){let sum=0;for(let i=0;i<50000000;i++)sum=(sum+i)>>>0;return sum;};</script>").unwrap();
    let driver = area.path().join("driver.cjs");
    fs::write(&driver,"const write=process.stdout.write.bind(process.stdout);write('HUNIN_READY\\n');delete process.env.HUNIN_CONTROL_TOKEN;delete process.env.HUNIN_FINALIZE_GATE;process.stdout.write=(chunk,...rest)=>write(String(chunk).startsWith('HUNIN_')?'driver:'+String(chunk):chunk,...rest);module.exports={input:async()=>{write('HUNIN_WORK_COMPLETE\\n');return null;},output:async value=>value};\n").unwrap();
    let project = Project::create(area.path(), "p").unwrap();
    let investigation = project
        .create_investigation("i", "browser stdout boundary", json!({}))
        .unwrap();
    let run = project.create_run(&investigation, "r", json!({})).unwrap();
    let plan = hunin_app::BrowserCapturePlan {
        request: RequestId {
            project: "p".into(),
            caller: "test".into(),
            id: "capture".into(),
        },
        run,
        kind: hunin_runtimes::browser::BrowserKind::Chromium,
        browser: PathBuf::from(std::env::var_os("HUNIN_CHROME").expect("selected headless Chrome")),
        node: PathBuf::from(std::env::var_os("HUNIN_NODE").expect("selected Node")),
        driver: None,
        driver_script: Some(driver),
        workload,
        conditions: json!({}),
        environment: BTreeMap::new(),
        read_paths: vec![],
        write_paths: vec![],
        safari_scripts: vec![],
        babel_root: None,
        babel_node: None,
    };
    let BeginCapture::Started(mut capture) = hunin_app::begin_browser(project, plan).unwrap()
    else {
        panic!("fresh browser capture")
    };
    capture.release().unwrap();
    let report = capture.finish(Duration::from_secs(20)).unwrap();
    assert!(report.application_success, "{}", report.application);
    assert_eq!(report.request.state, "succeeded");
    assert_eq!(
        report.application["output"],
        50_000_000u64 * 49_999_999 / 2 % (1u64 << 32)
    );
    let helper_elapsed = nanoseconds(&report.application["workEndNs"])
        - nanoseconds(&report.application["workStartNs"]);
    let controller_elapsed = report.elapsed_ns.parse::<u64>().unwrap();
    assert!(
        controller_elapsed + 2_000_000 >= helper_elapsed,
        "browser driver ordinary stdout prematurely ended the actual work window"
    );
    assert!(!report.code_sources.is_empty());
    eprintln!(
        "{}",
        json!({"family":"chromium-driver","controller_elapsed_ns":controller_elapsed,"helper_elapsed_ns":helper_elapsed,"collected_cpu_ns":report.cpu_ns,"resource_observations":report.resource_observations,"output":report.application["output"]})
    );
}
