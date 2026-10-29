//! Opt-in deterministic sizing inputs. Timings are observations, not CI budgets.
use hunin::{model::runs::Run, query, storage::Store};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::time::Instant;

#[test]
#[ignore = "opt-in local data-path sizing; choose HUNIN_SIZING_CLASS"]
fn selected_recording_data_paths() {
    let class = std::env::var("HUNIN_SIZING_CLASS").unwrap_or_else(|_| "small".into());
    let (samples, depth, threads, rounds, replace) = match class.as_str() {
        "small" => (128, 8, 1, 1, false),
        "wide" => (6000, 16, 8, 1, false),
        "deep" => (128, 512, 2, 1, false),
        "many-results" => (500, 8, 4, 16, false),
        "history" => (500, 8, 4, 16, true),
        _ => panic!("unknown sizing class: {class}"),
    };
    let root = tempfile::tempdir().unwrap();
    let store = Store::open(root.path()).unwrap();
    store
        .save_run(&Run {
            id: "sizing".into(),
            name: class.clone(),
            status: "completed".into(),
            command: vec!["synthetic-stack-scaling-v1".into()],
            cwd: ".".into(),
            runtime: "synthetic".into(),
            started_at_us: 1,
            finished_at_us: Some(2),
            exit_code: Some(0),
            diagnostics: vec![],
            source: json!({"fixture":"synthetic-stack-scaling-v1","fingerprint":"fixture-v1"}),
            correctness: json!({"passed":true}),
        })
        .unwrap();
    let frames: Vec<Value> = (0..depth)
        .map(|index| {
            json!({
                "function":format!("function_{index}"), "file":"fixture.ts", "line":index+1,
            })
        })
        .collect();
    let mut bytes = 0;
    let mut input_hash = Sha256::new();
    let mut selected = None;
    let phase = Instant::now();
    for round in 0..rounds {
        let records: Vec<_> = (0..samples)
            .map(|index| {
                json!({
                    "schema_version":1,"kind":"sample","id":format!("sample-{round}-{index}"),
                    "run_id":"sizing","producer_id":"fixture","pid":1,
                    "tid":format!("thread-{}",index%threads),"timestamp_us":index,
                    "basis":"sampled","stack":frames,"weight":1.0,"weight_unit":"samples",
                })
            })
            .collect();
        for record in &records {
            let encoded = serde_json::to_vec(record).unwrap();
            bytes += encoded.len();
            input_hash.update(&encoded);
            input_hash.update(b"\n");
        }
        let source = if replace {
            "raw".to_owned()
        } else {
            format!("source-{round}")
        };
        store
            .accept_import("sizing", &source, &format!("version-{round}"), &records)
            .unwrap();
        selected = store.selected_result_id("sizing", &source).unwrap();
    }
    let acceptance_us = phase.elapsed().as_micros();
    let phase = Instant::now();
    let records = store
        .records_for_result_ids("sizing", &[selected.unwrap()])
        .unwrap();
    let selected_read_us = phase.elapsed().as_micros();
    assert_eq!(records.len(), samples);
    let selected_bytes = serde_json::to_vec(&records).unwrap().len();
    let phase = Instant::now();
    let hotspots = query::hotspots(&records, "samples", 20);
    let hotspots_us = phase.elapsed().as_micros();
    assert_eq!(query::summary(&records)["total_weight"], samples as f64);
    let response_bytes = serde_json::to_vec(&hotspots).unwrap().len();
    drop(records);
    let phase = Instant::now();
    let snapshot = store.records_snapshot("sizing").unwrap();
    let all_selected_read_us = phase.elapsed().as_micros();
    assert_eq!(
        snapshot.records.len(),
        if replace { samples } else { samples * rounds }
    );
    // Optional copies make exact disposable datasets available to the analyzer
    // sizing control. Refuse an existing destination rather than replacing it.
    if let Some(destination) = std::env::var_os("HUNIN_SIZING_STORE") {
        let destination = std::path::PathBuf::from(destination);
        std::fs::create_dir(&destination).unwrap();
        std::fs::copy(
            root.path().join("hunin.sqlite"),
            destination.join("hunin.sqlite"),
        )
        .unwrap();
    }
    println!(
        "{}",
        json!({
            "fixture":"synthetic-stack-scaling-v1","class":class,"host_os":std::env::consts::OS,
            "host_arch":std::env::consts::ARCH,"debug_assertions":cfg!(debug_assertions),
            "source_revision":std::env::var("HUNIN_SIZING_REVISION").ok(),
            "samples_per_result":samples,"stack_depth":depth,"threads":threads,
            "result_versions":rounds,"replace_source":replace,
            "serialized_observation_bytes":bytes,"observation_stream_sha256":format!("{:x}",input_hash.finalize()),
            "selected_bytes":selected_bytes,"response_bytes":response_bytes,
            "database_bytes":std::fs::metadata(root.path().join("hunin.sqlite")).unwrap().len(),
            "timings_us":{"acceptance":acceptance_us,"selected_read":selected_read_us,
                "hotspots":hotspots_us,"all_selected_read":all_selected_read_us},
        })
    );
}
