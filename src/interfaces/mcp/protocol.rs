//! JSON-RPC negotiation, method validation and shared-operation adaptation.
use crate::operations::{Operations, catalog};
use serde_json::{Value, json};

pub async fn handle(operations: &Operations, message: &Value, initialized: bool) -> Value {
    let id = message.get("id").cloned().unwrap_or(Value::Null);
    let error = |code: i32, text: &str| json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":text}});
    if !id.is_null() && !id.is_string() && !id.is_number() {
        return json!({"jsonrpc":"2.0","id":null,"error":{"code":-32600,"message":"Invalid request ID"}});
    }
    if message["jsonrpc"] != "2.0" || !message["method"].is_string() {
        return error(-32600, "Invalid Request");
    }
    let method = message["method"].as_str().unwrap();
    if method == "initialize"
        && (!message["params"]["protocolVersion"].is_string()
            || !message["params"]["capabilities"].is_object()
            || !message["params"]["clientInfo"].is_object())
    {
        return error(
            -32602,
            "Initialization requires protocolVersion, capabilities and clientInfo",
        );
    }
    let result = match method {
        "initialize" => {
            json!({"protocolVersion":"2025-06-18","capabilities":{"tools":{}},"serverInfo":{"name":"hunin","version":env!("CARGO_PKG_VERSION")},"instructions":"Collect, inspect and compare development recordings. Sample weights are not exact call durations. Query capabilities before recording; preserve unresolved evidence."})
        }
        "ping" => json!({}),
        _ if !initialized => return error(-32000, "Initialize the MCP session first"),
        "tools/list" => json!({"tools":catalog()}),
        "tools/call" => {
            let Some(name) = message["params"]["name"].as_str() else {
                return error(-32602, "Tool name required");
            };
            let params = message["params"]
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            match operations.call(name, params).await {
                Ok(value) => {
                    json!({"content":[{"type":"text","text":value.to_string()}],"structuredContent":value,"isError":false})
                }
                Err(e) => {
                    json!({"content":[{"type":"text","text":format!("{e:#}")}],"isError":true,"_meta":{"hunin/operation-error":crate::operations::operation_failure(name, &e)}})
                }
            }
        }
        _ => return error(-32601, "Method not found"),
    };
    json!({"jsonrpc":"2.0","id":id,"result":result})
}
