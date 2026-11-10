use hunin_runtimes::{Limits, browser::safari};
use std::{fs, path::PathBuf, process::Command, time::Duration};
#[test]
#[ignore = "requires HUNIN_BABEL_NODE and HUNIN_BABEL_ROOT"]
fn generic_function_instrumentation_preserves_checked_completion_and_declares_loss() {
    let node = PathBuf::from(std::env::var("HUNIN_BABEL_NODE").unwrap());
    let babel = PathBuf::from(std::env::var("HUNIN_BABEL_ROOT").unwrap());
    let area = tempfile::tempdir().unwrap();
    let fixture =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/safari-semantics.js");
    let instrumented = area.path().join("observed.js");
    let binding = area.path().join("observed.json");
    safari::instrument(
        &node,
        &babel,
        &fixture,
        &instrumented,
        &binding,
        area.path(),
    )
    .unwrap();
    // the second declared script reuses only the exact owned compiler implementation.
    safari::instrument(
        &node,
        &babel,
        &fixture,
        &area.path().join("second.js"),
        &area.path().join("second.json"),
        area.path(),
    )
    .unwrap();
    let runner = area.path().join("probe.cjs");
    fs::write(&runner,r#"const fs=require('node:fs'),vm=require('node:vm'),{performance}=require('node:perf_hooks');
(async()=>{const [original,instrumented,binding,output]=process.argv.slice(2);const meta=JSON.parse(fs.readFileSync(binding));const source=fs.readFileSync(original,'utf8'),observed=fs.readFileSync(instrumented,'utf8');const baseline=vm.createContext({performance});vm.runInContext(source,baseline);const expected=await baseline.run();const context=vm.createContext({performance});vm.runInContext(observed,context);context.unrelated();context.__HuninObservation.start({bindings:[meta],max_events:10000});const result=await context.run();const profile=context.__HuninObservation.stop();context.unrelated();const limitedContext=vm.createContext({performance});vm.runInContext(observed,limitedContext);limitedContext.__HuninObservation.start({bindings:[meta],max_events:4});const limitedResult=await limitedContext.run();const limited=limitedContext.__HuninObservation.stop();if(JSON.stringify(expected)!==JSON.stringify(result)||JSON.stringify(expected)!==JSON.stringify(limitedResult))throw Error('changed application completion');fs.writeFileSync(output,JSON.stringify({expected,result,profile,limited}));})().catch(error=>{console.error(error);process.exitCode=1});"#).unwrap();
    let output = area.path().join("result.json");
    let mut command = Command::new(&node);
    command.args([runner.as_path(), &fixture, &instrumented, &binding, &output]);
    hunin_platforms::tools::require_success(
        hunin_platforms::tools::run(
            &mut command,
            area.path(),
            Duration::from_secs(20),
            64 * 1024,
        )
        .unwrap(),
    )
    .unwrap();
    let result: serde_json::Value = serde_json::from_slice(&fs::read(output).unwrap()).unwrap();
    assert_eq!(
        result["result"],
        serde_json::json!({"method":24,"total":15,"recur":4,"caught":true,"override":17,"strict":true,"lexical":true,"delayed":7,"first":3,"returned":23,"generatorFinally":1,"rejection":true})
    );
    let profile = safari::extract(
        &serde_json::to_vec(&result["profile"]).unwrap(),
        "page",
        Limits::default(),
    )
    .unwrap();
    assert!(profile.capture_complete);
    assert!(profile.records.iter().any(|r| r.schema == "safari.call"
        && r.function.as_deref() == Some("fail")
        && r.payload["kind"] == "exit"
        && r.payload["body_throw"] == true));
    assert!(
        !profile
            .records
            .iter()
            .any(|r| r.schema == "safari.call" && r.function.as_deref() == Some("unrelated"))
    );
    assert!(profile.records.iter().any(|r| r.schema == "safari.call"
        && r.function.as_deref() == Some("returnsRejected")
        && r.payload["kind"] == "exit"
        && r.payload["body_throw"] == false));
    let limited = safari::extract(
        &serde_json::to_vec(&result["limited"]).unwrap(),
        "page",
        Limits::default(),
    )
    .unwrap();
    assert!(!limited.capture_complete);
    assert!(result["limited"]["lost_events"].as_u64().unwrap() > 0);
    for (name, text) in [
        ("eval.js", "function unsafe(){return eval('1+1')}"),
        ("with.js", "with({x:1}){x}"),
        ("shadow.js", "const globalThis={};function f(){return 1}"),
    ] {
        let file = area.path().join(name);
        fs::write(&file, text).unwrap();
        assert!(
            safari::instrument(
                &node,
                &babel,
                &file,
                &area.path().join(format!("{name}.out.js")),
                &area.path().join(format!("{name}.json")),
                area.path()
            )
            .is_err()
        );
    }
}

#[test]
#[ignore = "requires HUNIN_BABEL_NODE and HUNIN_BABEL_ROOT"]
fn suspended_bodies_cannot_close_or_mark_loss_in_a_later_capture() {
    let node = PathBuf::from(std::env::var("HUNIN_BABEL_NODE").unwrap());
    let babel = PathBuf::from(std::env::var("HUNIN_BABEL_ROOT").unwrap());
    let area = tempfile::tempdir().unwrap();
    let source = area.path().join("suspension.js");
    fs::write(&source, "function first(){};async function waitA(){await new Promise(resolve=>globalThis.resumeA=resolve);return 1};async function waitB(){await new Promise(resolve=>globalThis.resumeB=resolve);return 2}").unwrap();
    let script = area.path().join("instrumented.js");
    let binding = area.path().join("binding.json");
    safari::instrument(&node, &babel, &source, &script, &binding, area.path()).unwrap();
    let runner = area.path().join("run.cjs");
    fs::write(&runner,r#"const vm=require('node:vm'),fs=require('node:fs'),{performance}=require('node:perf_hooks');
(async()=>{const [script,binding,out]=process.argv.slice(2),source=fs.readFileSync(script,'utf8'),meta=JSON.parse(fs.readFileSync(binding));const results=[];for(const maximum of [10000,1]){const page=vm.createContext({performance});vm.runInContext(source,page);page.__HuninObservation.start({bindings:[meta],max_events:maximum});page.first();const a=page.waitA();const old=page.__HuninObservation.stop();const ack=page.__HuninObservation.start({bindings:[meta]});if(!ack.active||ack.script_count!==1)throw Error('missing start acknowledgement');const b=page.waitB();page.resumeA();if(await a!==1)throw Error('changed first result');page.resumeB();if(await b!==2)throw Error('changed second result');results.push({old,current:page.__HuninObservation.stop()});}fs.writeFileSync(out,JSON.stringify(results));})().catch(e=>{console.error(e);process.exitCode=1});"#).unwrap();
    let output = area.path().join("profiles.json");
    let mut command = Command::new(&node);
    command.args([runner.as_path(), &script, &binding, &output]);
    hunin_platforms::tools::require_success(
        hunin_platforms::tools::run(
            &mut command,
            area.path(),
            Duration::from_secs(20),
            64 * 1024,
        )
        .unwrap(),
    )
    .unwrap();
    let profiles: Vec<serde_json::Value> =
        serde_json::from_slice(&fs::read(output).unwrap()).unwrap();
    for value in profiles {
        let current = safari::extract(
            &serde_json::to_vec(&value["current"]).unwrap(),
            "page",
            Limits::default(),
        )
        .unwrap();
        assert!(current.capture_complete);
        assert!(current.records.iter().any(|r| r.schema == "safari.call"
            && r.function.as_deref() == Some("waitB")
            && r.payload["kind"] == "exit"));
        assert!(
            !current
                .records
                .iter()
                .any(|r| r.schema == "safari.call" && r.function.as_deref() == Some("waitA"))
        );
        assert_eq!(value["current"]["lost_events"], 0);
    }
}
