use super::{compiler, ll, raw, wasm};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};

#[test]
fn compiler_range_inversion_obeys_the_shared_work_budget_before_expanding_a_tiny_export() {
    use crate::importers::output::{Limits, OutputBudget, RecordBatch};
    let trace = json!({"phases":[{"type":"graph","data":{"nodes":[{"id":1,"origin":{"bytecodePosition":5}}]}},{"nodeIdToInstructionRange":{"1":[0,100000]}}]});
    let mut budget = OutputBudget::with_limits(Limits {
        rows: 100,
        bytes: 1_000_000,
        record_bytes: 1_000_000,
        frame_visits: 4,
        attachment_bytes: 1_000_000,
    });
    let mut records = RecordBatch::new(&mut budget, None);
    let error = compiler::selected_origins(&trace, &[], &mut records).unwrap_err();
    assert!(
        error
            .downcast_ref::<crate::importers::output::NormalizationOutputLimit>()
            .is_some()
    );
}

#[test]
fn return_address_boundary_keeps_the_callsite_generation_separate_from_the_next_range() {
    let log=raw::parse("v8-version,12,4,254,21,-node.26,0\ncode-creation,JS,4,10,0x1000,8,caller\ncode-creation,JS,4,11,0x1008,8,next_function\ntick,0x1000,12,0,0x0,0,0x1008\n").unwrap();
    assert_eq!(log.ticks[0].frames[1].address, 0x1008);
    assert_eq!(log.ticks[0].frames[1].code, Some(1));
    assert_eq!(log.ticks[0].frames[1].arm64_callsite_code, Some(0));
}

#[test]
fn raw_frame_allocation_reserves_work_before_materializing_a_stack() {
    use crate::importers::output::{Limits, OutputBudget, RecordBatch};
    let mut budget = OutputBudget::with_limits(Limits {
        rows: 100,
        bytes: 1_000_000,
        record_bytes: 1_000_000,
        frame_visits: 2,
        attachment_bytes: 1_000_000,
    });
    let mut records = RecordBatch::new(&mut budget, None);
    let error = raw::parse_controlled(
        "v8-version,12,4,254,21,-node.26,0\ntick,0x1000,1,0,0x0,0,0x2000,0x3000\n",
        |visit| match visit {
            raw::Visit::Record => records.check(),
            raw::Visit::Frame => records.frame(),
        },
    )
    .unwrap_err();
    assert!(
        error
            .downcast_ref::<crate::importers::output::NormalizationOutputLimit>()
            .is_some()
    );
}

#[test]
fn compiler_body_join_checks_actual_opcode_boundaries_names_and_every_immediate_byte() {
    let module = [
        0, 97, 115, 109, 1, 0, 0, 0, 1, 7, 1, 96, 2, 127, 127, 1, 127, 3, 2, 1, 0, 7, 7, 1, 3, 97,
        100, 100, 0, 0, 10, 9, 1, 7, 0, 32, 0, 32, 1, 106, 11,
    ];
    let function = wasm::functions(&module).unwrap().remove(0);
    let mut trace = json!({"source":"0x00,\n// body:\nkExprLocalGet, 0x00, // local.get\nkExprLocalGet, 0x01, // local.get\nkExprI32Add, // i32.add\nkExprEnd, // end\n","sourceLineToBytecodePosition":[-1,-1,1,3,5,6]});
    assert!(compiler::source_matches(&trace, &function).unwrap());
    let mut duplicate = trace.clone();
    duplicate["sourceLineToBytecodePosition"] = json!([-1, -1, 1, 1, 5, 6]);
    duplicate["source"] = json!(
        duplicate["source"]
            .as_str()
            .unwrap()
            .replace("0x01", "0x00")
    );
    assert!(!compiler::source_matches(&duplicate, &function).unwrap());
    trace["source"] = json!(trace["source"].as_str().unwrap().replace("0x01", "0x02"));
    assert!(!compiler::source_matches(&trace, &function).unwrap());
    trace["source"] = json!(
        trace["source"]
            .as_str()
            .unwrap()
            .replace("0x02", "0x01")
            .replace("i32.add", "i32.sub")
    );
    assert!(!compiler::source_matches(&trace, &function).unwrap());
}

#[test]
fn machine_identity_only_accepts_exact_bytes_and_declared_arm64_call_relocations() {
    let temporary = BTreeMap::from([(0, 0x8b010000u32), (4, 0x94000001)]);
    let published = BTreeMap::from([(0, 0x8b010000u32), (4, 0x94000002)]);
    let bytes = published
        .values()
        .flat_map(|word| word.to_le_bytes())
        .collect::<Vec<_>>();
    assert!(compiler::relocated_words_match(
        &temporary,
        &published,
        &BTreeSet::from([4]),
        &bytes
    ));
    assert!(!compiler::relocated_words_match(
        &temporary,
        &published,
        &BTreeSet::new(),
        &bytes
    ));
    let wrong = BTreeMap::from([(0, 0x8b020000u32), (4, 0x94000002)]);
    assert!(!compiler::relocated_words_match(
        &temporary,
        &wrong,
        &BTreeSet::from([0, 4]),
        &bytes
    ));
    let mut wrong = bytes.clone();
    wrong[0] ^= 1;
    assert!(!compiler::relocated_words_match(
        &temporary,
        &published,
        &BTreeSet::from([4]),
        &wrong
    ));
    let mut longer = bytes.clone();
    longer.extend(0u32.to_le_bytes());
    assert!(!compiler::relocated_words_match(
        &temporary,
        &published,
        &BTreeSet::from([4]),
        &longer
    ));
}

#[test]
fn wasm_function_coordinates_include_locals_but_origins_require_actual_operator_boundaries() {
    let module = [
        0, 97, 115, 109, 1, 0, 0, 0, 1, 7, 1, 96, 2, 127, 127, 1, 127, 3, 2, 1, 0, 7, 7, 1, 3, 97,
        100, 100, 0, 0, 10, 9, 1, 7, 0, 32, 0, 32, 1, 106, 11,
    ];
    let functions = wasm::functions(&module).unwrap();
    assert_eq!(functions.len(), 1);
    assert_eq!(functions[0].index, 0);
    assert_eq!(functions[0].body_start, 34);
    assert_eq!(functions[0].body, [0, 32, 0, 32, 1, 106, 11]);
    assert!(!functions[0].operations.contains_key(&0));
    assert_eq!(functions[0].operations[&5], (6, "I32Add".into()));
    assert_eq!(functions[0].label, "add");
    assert!(wasm::functions(&module[..module.len() - 1]).is_err());
}

#[test]
fn raw_ticks_resolve_generation_at_event_order_not_final_address_dictionary() {
    let log = raw::parse(concat!(
        "v8-version,12,4,254,21,-node.26,0\n",
        "code-creation,Wasm,0,10,0x1000,16,same_name\n",
        "tick,0x1004,11,0,0x0,0,0x1008\n",
        "code-move,0x1000,0x2000\n",
        "tick,0x2004,12,0,0x0,0\n",
        "code-delete,0x2000\n",
        "code-creation,Wasm,0,13,0x1000,8,same_name\n",
        "tick,0x1004,14,0,0x0,0,0x4000\n",
    ))
    .unwrap();
    assert_eq!(log.version, "12.4.254.21-node.26");
    assert_eq!(log.codes.len(), 3);
    assert_eq!(log.codes[1].address, 0x2000);
    assert_eq!(log.ticks[0].frames[0].code, Some(0));
    assert_eq!(log.ticks[1].frames[0].code, Some(1));
    assert_eq!(log.ticks[2].frames[0].code, Some(2));
    assert_eq!(log.ticks[2].frames[1].code, None);
    assert!(log.codes[0].end_censored);
    assert!(log.codes[1].start_censored);
    assert_ne!(log.codes[0].generation, log.codes[2].generation);
}

#[test]
fn malformed_known_records_fail_and_unknown_records_remain_diagnostic() {
    let field_bomb = format!(
        "v8-version,12,4,254,21,-node.26,0\ntick,{}\n",
        "0,".repeat(10000)
    );
    assert!(
        raw::parse(&field_bomb)
            .unwrap_err()
            .to_string()
            .contains("record field limit")
    );
    for text in [
        "v8-version,12,4,254,21,-node.26,0\ncode-creation,Wasm,0,1,0xffffffffffffffff,4,x\n",
        "v8-version,12,4,254,21,-node.26,0\ntick,0x1000,9007199254740992,0,0x0,0\n",
        "v8-version,12,4,254,21,-node.26,0\ntick,not-an-address,1,0,0x0,0\n",
    ] {
        assert!(raw::parse(text).is_err());
    }
    let log = raw::parse(
        "v8-version,12,4,254,21,-node.26,0\nfuture-record,a,b\ntick,0x1000,1,0,0x0,0,overflow\n",
    )
    .unwrap();
    assert!(log.ticks[0].overflow);
    assert!(log.diagnostics.iter().any(|v| v.contains("future-record")));
}

fn low_level() -> Vec<u8> {
    let mut bytes = b"arm64\0C".to_vec();
    bytes.extend(4i32.to_le_bytes());
    bytes.extend([0; 4]);
    bytes.extend(0x1000u64.to_le_bytes());
    bytes.extend(8i32.to_le_bytes());
    bytes.extend([0; 4]);
    bytes.extend(b"demo");
    bytes.extend([1, 2, 3, 4, 5, 6, 7, 8]);
    bytes.push(b'M');
    bytes.extend(0x1000u64.to_le_bytes());
    bytes.extend(0x2000u64.to_le_bytes());
    bytes.push(b'G');
    bytes
}

#[test]
fn low_level_retains_exact_byte_extent_and_rejects_truncation_or_unknown_abi() {
    let bytes = low_level();
    let code = ll::parse(&bytes, "arm64").unwrap();
    assert_eq!(code[0].address, 0x1000);
    assert_eq!(code[0].name, "demo");
    assert_eq!(code[0].content, [1, 2, 3, 4, 5, 6, 7, 8]);
    assert_eq!(
        &bytes[code[0].offset as usize..code[0].offset as usize + 8],
        code[0].content
    );
    assert!(ll::parse(&bytes[..bytes.len() - 2], "arm64").is_err());
    assert!(ll::parse(&bytes, "x64").is_err());
    let mut unknown = bytes.clone();
    unknown.push(b'?');
    assert!(ll::parse(&unknown, "arm64").is_err());
}

#[test]
fn normalized_native_wasm_proofs_survive_concatenated_exports_and_pass_public_ingestion() {
    use crate::importers::output::{OutputBudget, RecordBatch};
    use std::fs;
    let directory = tempfile::tempdir().unwrap();
    let module = [
        0, 97, 115, 109, 1, 0, 0, 0, 1, 7, 1, 96, 2, 127, 127, 1, 127, 3, 2, 1, 0, 7, 7, 1, 3, 97,
        100, 100, 0, 0, 10, 9, 1, 7, 0, 32, 0, 32, 1, 106, 11,
    ];
    fs::write(directory.path().join("module.wasm"), module).unwrap();
    fs::write(directory.path().join("native.log"),"v8-version,12,4,254,21,-node.26,0\ncode-creation,JS,4,10,0x1000,8,add\ntick,0x1000,11,0,0x0,0\ntick,0x1004,12,0,0x0,0\ncode-creation,JS,4,13,0x2000,8,add\ntick,0x2000,14,0,0x0,0\n").unwrap();
    let mut ll = b"arm64\0C".to_vec();
    ll.extend(3i32.to_le_bytes());
    ll.extend([0; 4]);
    ll.extend(0x1000u64.to_le_bytes());
    ll.extend(8i32.to_le_bytes());
    ll.extend([0; 4]);
    ll.extend(b"add");
    ll.extend(0x8b010000u32.to_le_bytes());
    ll.extend(0xd65f03c0u32.to_le_bytes());
    ll.push(b'C');
    ll.extend(3i32.to_le_bytes());
    ll.extend([0; 4]);
    ll.extend(0x2000u64.to_le_bytes());
    ll.extend(8i32.to_le_bytes());
    ll.extend([0; 4]);
    ll.extend(b"add");
    ll.extend(0x8b020000u32.to_le_bytes());
    ll.extend(0xd65f03c0u32.to_le_bytes());
    fs::write(directory.path().join("native.log.ll"), &ll).unwrap();
    fs::write(directory.path().join("stdout.txt"),"--- WebAssembly code ---\nindex: 0\ncompiler: TurboFan\nBody (size = 8)\nInstructions (size = 8)\n0x1000 0 8b010000 add x0,x0,x1\n0x1004 4 d65f03c0 ret\nRelocInfo (size = 0)\n--- End code ---\n").unwrap();
    let trace = json!({"function":"add","source":"0x00,\n// body:\nkExprLocalGet, 0x00, // local.get\nkExprLocalGet, 0x01, // local.get\nkExprI32Add, // i32.add\nkExprEnd, // end\n","sourceLineToBytecodePosition":[-1,-1,1,3,5,6],"phases":[
        {"type":"graph","data":{"nodes":[{"id":1,"origin":{"bytecodePosition":5}}]}},
        {"type":"instructions","nodeIdToInstructionRange":{"1":[0,1]}},
        {"name":"after register allocation","blocks":[{"instructions":[{"id":0,"flags":""}]}]},
        {"instructionOffsetToPCOffset":{"0":{"gap":0,"arch":0,"condition":4}},"codeOffsetsInfo":{"outOfLineCode":8}},
        {"type":"disassembly","data":"0x9000 0 8b010000 add x0,x0,x1\n0x9004 4 d65f03c0 ret\n"}
    ]});
    let document = serde_json::to_string(&trace).unwrap();
    fs::write(
        directory.path().join("compiler.json"),
        format!("{document}{document}{{"),
    )
    .unwrap();
    let reference = super::evidence::reference("module.wasm", &module, "profile-sidecar");
    let manifest = json!({"schema_version":1,"pid":7,"runtime":{"v8":"12.4.254.21-node.26","arch":"arm64"},"max_artifact_bytes":65536,
        "logs":[{"path":"native.log","ll_path":"native.log.ll"}],"wasm":[{"path":"module.wasm","sha256":reference.sha256}],
        "compiler_exports":[{"path":"compiler.json"}],"published_code":{"path":"stdout.txt"}});
    let mut budget = OutputBudget::default();
    let mut batch = RecordBatch::new(&mut budget, None);
    let diagnostics = super::decode_into(
        &directory.path().join("capture.v8code.json"),
        &manifest,
        "run",
        "key",
        &mut batch,
    )
    .unwrap();
    let records = batch.finish().unwrap();
    crate::ingestion::validate("run", &records, &Default::default()).unwrap();
    let mappings = records
        .iter()
        .filter(|r| r["relation"] == "code_maps_to")
        .collect::<Vec<_>>();
    assert_eq!(mappings.len(), 1);
    assert_eq!(
        mappings[0]["attributes"]["code_mapping"]["to_range"]["start"],
        "0x5"
    );
    assert_eq!(
        mappings[0]["attributes"]["code_mapping"]["from_range"]["end_exclusive"],
        "0x1004"
    );
    assert_eq!(mappings[0]["evidence"].as_array().unwrap().len(), 2);
    assert!(
        diagnostics
            .iter()
            .any(|line| line.contains("incomplete or unsupported JSON suffix"))
    );
    let samples = records
        .iter()
        .filter(|r| r["kind"] == "sample")
        .collect::<Vec<_>>();
    assert_eq!(samples.len(), 3);
    assert!(samples[0]["stack"][0]["code_id"].is_string());
    assert_ne!(
        samples[0]["stack"][0]["code_id"],
        samples[2]["stack"][0]["code_id"]
    );
    assert_eq!(
        crate::query::hotspots(&records, "samples", 10)["hotspots"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        samples[0]["attributes"]["clock_alignment"],
        "runtime_relative"
    );
    assert_eq!(samples[0]["stack"][0]["code_offset"], "0x1000");

    // One old byte export must not be borrowed by a later generation at the
    // same address, even when its size and reported name also happen to match.
    let raw_path = directory.path().join("native.log");
    let mut raw = fs::read_to_string(&raw_path).unwrap();
    raw.push_str(
        "code-delete,0x1000\ncode-creation,JS,4,15,0x1000,8,add\ntick,0x1000,16,0,0x0,0\n",
    );
    fs::write(raw_path, raw).unwrap();
    let mut budget = OutputBudget::default();
    let mut batch = RecordBatch::new(&mut budget, None);
    super::decode_into(
        &directory.path().join("capture.v8code.json"),
        &manifest,
        "run",
        "reused",
        &mut batch,
    )
    .unwrap();
    let reused = batch.finish().unwrap();
    for sample in reused
        .iter()
        .filter(|r| r["kind"] == "sample" && r["stack"][0]["address"] != "0x2000")
    {
        assert!(sample["stack"][0]["code_id"].is_null());
    }
    assert!(!reused.iter().any(|r| r["relation"] == "code_maps_to"));

    // A return address at B's start is not B's caller identity. Only a retained
    // call instruction in A permits attributing it to A.
    let caller_raw = "v8-version,12,4,254,21,-node.26,0\ncode-creation,JS,4,10,0x1000,8,caller\ncode-creation,JS,4,11,0x1008,8,next_function\ntick,0x1000,12,0,0x0,0,0x1008\n";
    fs::write(directory.path().join("native.log"), caller_raw).unwrap();
    let mut budget = OutputBudget::default();
    let mut batch = RecordBatch::new(&mut budget, None);
    super::decode_into(
        &directory.path().join("capture.v8code.json"),
        &manifest,
        "run",
        "return-unknown",
        &mut batch,
    )
    .unwrap();
    let unknown = batch.finish().unwrap();
    let sample = unknown.iter().find(|r| r["kind"] == "sample").unwrap();
    assert_eq!(
        sample["stack"][0]["function"],
        "[unresolved return address]"
    );
    assert!(sample["stack"][0]["code_id"].is_null());

    let offset = ll::parse(&ll, "arm64").unwrap()[0].offset as usize;
    ll[offset + 4..offset + 8].copy_from_slice(&0x94000000u32.to_le_bytes());
    fs::write(directory.path().join("native.log.ll"), &ll).unwrap();
    let mut budget = OutputBudget::default();
    let mut batch = RecordBatch::new(&mut budget, None);
    super::decode_into(
        &directory.path().join("capture.v8code.json"),
        &manifest,
        "run",
        "return-validated",
        &mut batch,
    )
    .unwrap();
    let validated = batch.finish().unwrap();
    let sample = validated.iter().find(|r| r["kind"] == "sample").unwrap();
    assert_eq!(sample["stack"][0]["function"], "caller");
    assert_eq!(sample["stack"][0]["address"], "0x1008");
    assert_eq!(sample["stack"][0]["code_offset"], "0x1004");
    assert_eq!(
        sample["stack"][0]["code_address_basis"],
        "validated_arm64_callsite_from_return_address"
    );
    assert!(sample["stack"][0]["code_id"].is_string());

    fs::write(
        directory.path().join("native.log"),
        caller_raw.replace("0,0x1008\n", "0,0x1009\n"),
    )
    .unwrap();
    let mut budget = OutputBudget::default();
    let mut batch = RecordBatch::new(&mut budget, None);
    super::decode_into(
        &directory.path().join("capture.v8code.json"),
        &manifest,
        "run",
        "return-unaligned",
        &mut batch,
    )
    .unwrap();
    let unaligned = batch.finish().unwrap();
    let sample = unaligned.iter().find(|r| r["kind"] == "sample").unwrap();
    assert_eq!(
        sample["stack"][0]["function"],
        "[unresolved return address]"
    );
}
