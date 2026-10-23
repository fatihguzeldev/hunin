use hunin_sdk::{counter, flush, Context};
#[test]
fn explicit_context_survives_thread_transfer() {
    let directory = std::env::temp_dir().join(format!("hunin-rust-sdk-{}", std::process::id()));
    std::env::set_var("HUNIN_RUN_ID", "test");
    std::env::set_var("HUNIN_RECORD_DIR", &directory);
    let context = Context::new("request");
    let parent = context.span("parent");
    let token = parent.context();
    std::thread::spawn(move || {
        let _child = token.span("child");
        counter(&token, "items", 1.0, "count");
    })
    .join()
    .unwrap();
    parent.end();
    flush().unwrap();
    let path = std::fs::read_dir(&directory)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let text = std::fs::read_to_string(path).unwrap();
    let rows: Vec<serde_json::Value> = text
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let parent = rows.iter().find(|row| row["name"] == "parent").unwrap();
    let child = rows.iter().find(|row| row["name"] == "child").unwrap();
    assert_eq!(child["parent_id"], parent["id"]);
    assert_eq!(child["context_id"], parent["context_id"]);
    std::fs::remove_dir_all(directory).unwrap();
}
