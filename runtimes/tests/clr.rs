
use hunin_runtimes::{Limits, clr};
use serde_json::{Value, json};

fn profile() -> Value {
    json!({"format":"hunin-eventpipe-v1","traceevent":"4.0.0","runtime":"10.0.15","frequency":10000000,"origin_qpc":50,"events_lost":0,"source_binding":{"matched_pdb":true},"warnings":[],"future_header":{"retained":true},"frames":[{"id":1,"address":u64::MAX,"function":"Preview.Resize","module":"preview.dll","module_pdb":null,"token":100663297,"il_offset":12,"source_file":"Preview.cs","source_line":8,"source_mapping":"matched portable-pdb il sequence point","future_frame":42}],"stacks":[{"id":0,"frame":1,"caller":null}],"events":[{"id":7,"provider":"Microsoft-DotNETCore-SampleProfiler","name":"Thread/Sample","provider_guid":"00000000-0000-0000-0000-000000000001","event_id":1,"version":0,"activity":"00000000-0000-0000-0000-000000000000","related_activity":"00000000-0000-0000-0000-000000000000","process":42,"thread":43,"time_qpc":51,"time_ns":100,"uncertainty_ns":100,"stack":0,"payload":{"counter":u64::MAX},"future_event":"retained"}]})
}

#[test]
fn eventpipe_retains_exact_ticks_frames_stack_edges_payload_and_loss() {
    let mut profile = profile();
    profile["events_lost"] = json!(9);
    let extracted = clr::extract(
        &serde_json::to_vec(&profile).unwrap(),
        "clr",
        Limits::default(),
    )
    .unwrap();
    let event = extracted
        .records
        .iter()
        .find(|record| record.schema == "eventpipe.event")
        .unwrap();
    assert_eq!(event.payload["payload"]["counter"], json!(u64::MAX));
    assert_eq!(event.payload["future_event"], "retained");
    let time = event.time.as_ref().unwrap();
    assert_eq!((time.start, time.uncertainty), (100, 100));
    assert_eq!(event.function.as_deref(), Some("Preview.Resize"));
    assert_eq!(event.location.as_deref(), Some("Preview.cs:8"));
    let frame = extracted
        .records
        .iter()
        .find(|record| record.schema == "eventpipe.frame")
        .unwrap();
    assert_eq!(frame.payload["address"], json!(u64::MAX));
    assert_eq!(frame.payload["future_frame"], 42);
    assert!(!extracted.warnings.is_empty());
    assert_eq!(
        extracted.records.last().unwrap().payload["future_header"]["retained"],
        true
    );
}

#[test]
fn eventpipe_rejects_mistaken_clocks_cycles_references_and_pressure() {
    let mut cases = Vec::new();
    let mut wrong = profile();
    wrong["events"][0]["time_ns"] = json!(99);
    cases.push(wrong);
    let mut wrong = profile();
    wrong["frequency"] = json!(0);
    cases.push(wrong);
    let mut wrong = profile();
    wrong["events"][0]["uncertainty_ns"] = json!(1);
    cases.push(wrong);
    let mut wrong = profile();
    wrong["stacks"][0]["caller"] = json!(0);
    cases.push(wrong);
    let mut wrong = profile();
    wrong["stacks"][0]["frame"] = json!(8);
    cases.push(wrong);
    let mut wrong = profile();
    wrong["events"][0]["stack"] = json!(8);
    cases.push(wrong);
    let mut wrong = profile();
    wrong["frames"][0]["source_line"] = Value::Null;
    cases.push(wrong);
    for wrong in cases {
        assert!(
            clr::extract(
                &serde_json::to_vec(&wrong).unwrap(),
                "clr",
                Limits::default()
            )
            .is_err()
        );
    }
    for limits in [
        Limits {
            max_input_bytes: 8,
            ..Limits::default()
        },
        Limits {
            max_records: 1,
            ..Limits::default()
        },
        Limits {
            max_output_bytes: 8,
            ..Limits::default()
        },
    ] {
        assert!(clr::extract(&serde_json::to_vec(&profile()).unwrap(), "clr", limits).is_err());
    }
}

#[test]
fn unresolved_eventpipe_symbols_keep_runtime_names_without_source_locations() {
    let mut profile = profile();
    profile["frames"][0]["source_file"] = Value::Null;
    profile["frames"][0]["source_line"] = Value::Null;
    profile["frames"][0]["source_mapping"] = json!("source checksum mismatch");
    let extraction = clr::extract(
        &serde_json::to_vec(&profile).unwrap(),
        "clr",
        Limits::default(),
    )
    .unwrap();
    let frame = extraction
        .records
        .iter()
        .find(|record| record.schema == "eventpipe.frame")
        .unwrap();
    assert_eq!(frame.function.as_deref(), Some("Preview.Resize"));
    assert!(frame.location.is_none());
}

