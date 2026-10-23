use hunin::storage::Store;
use serde_json::{Value, json};
use std::{process::Stdio, time::Duration};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

#[tokio::test]
async fn mcp_eof_finalizes_its_owned_live_recording() {
    let project = tempfile::tempdir().unwrap();
    let data = tempfile::tempdir().unwrap();
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_hunin"))
        .args(["--data-dir", data.path().to_str().unwrap(), "mcp"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap()).lines();
    input.write_all(format!("{}\n",json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"lifecycle-control","version":"1"}}})).as_bytes()).await.unwrap();
    let initialized: Value = serde_json::from_str(
        &tokio::time::timeout(Duration::from_secs(10), output.next_line())
            .await
            .unwrap()
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(initialized["id"], 1);
    input.write_all(format!("{}\n{}\n",json!({"jsonrpc":"2.0","method":"notifications/initialized"}),json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"runs.start","arguments":{"runtime":"none","cwd":project.path(),"command":["node","-e","setInterval(()=>{},1000)"]}}})).as_bytes()).await.unwrap();
    let started: Value = serde_json::from_str(
        &tokio::time::timeout(Duration::from_secs(10), output.next_line())
            .await
            .unwrap()
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(started["result"]["isError"], false, "{started}");
    let id = started["result"]["structuredContent"]["run"]["id"]
        .as_str()
        .unwrap();
    let store = Store::open(data.path()).unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        while store.run(id).unwrap().source["supervised_pid"].is_null() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    drop(input);
    let status = tokio::time::timeout(Duration::from_secs(10), child.wait())
        .await
        .unwrap()
        .unwrap();
    assert!(status.success());
    let recording = store.run(id).unwrap();
    assert_eq!(recording.status, "canceled");
    assert!(recording.finished_at_us.is_some());
}
