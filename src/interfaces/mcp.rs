use crate::operations::{Operations, catalog};
use anyhow::Result;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

pub async fn serve(operations: Operations) -> Result<()> {
    let result = serve_session(operations.clone()).await;
    let shutdown = operations.shutdown().await;
    result?;
    shutdown
}

async fn serve_session(operations: Operations) -> Result<()> {
    let mut input = BufReader::new(tokio::io::stdin());
    let output = std::sync::Arc::new(tokio::sync::Mutex::new(tokio::io::stdout()));
    let mut initialized = false;
    let mut negotiated = false;
    let mut requests = tokio::task::JoinSet::new();
    loop {
        let message = tokio::select! {
            message = read_message(&mut input) => message?,
            _ = tokio::signal::ctrl_c() => None,
        };
        let Some((line, too_large)) = message else {
            break;
        };
        if too_large {
            write_response(&output,&json!({"jsonrpc":"2.0","id":null,"error":{"code":-32600,"message":"Message exceeds 1 MiB limit"}})).await?;
            continue;
        }
        let message: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => {
                write_response(&output,&json!({"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":"Parse error"}})).await?;
                continue;
            }
        };
        if !message.is_object() {
            write_response(&output,&json!({"jsonrpc":"2.0","id":null,"error":{"code":-32600,"message":"Invalid Request"}})).await?;
            continue;
        }
        if message["method"] == "notifications/initialized" {
            if negotiated {
                initialized = true;
            }
            continue;
        }
        if message.get("id").is_none() {
            continue;
        }
        if message["method"] == "initialize" {
            let response = if negotiated {
                json!({"jsonrpc":"2.0","id":message["id"],"error":{"code":-32600,"message":"Session already initialized"}})
            } else {
                handle(&operations, &message, false).await
            };
            negotiated = response.get("result").is_some();
            write_response(&output, &response).await?;
            continue;
        }
        let operations = operations.clone();
        let output = output.clone();
        requests.spawn(async move {
            let response = handle(&operations, &message, initialized).await;
            write_response(&output, &response).await
        });
        while let Some(result) = requests.try_join_next() {
            result??;
        }
    }
    while let Some(result) = requests.join_next().await {
        result??;
    }
    Ok(())
}

async fn write_response(
    output: &tokio::sync::Mutex<tokio::io::Stdout>,
    response: &Value,
) -> Result<()> {
    let mut output = output.lock().await;
    output
        .write_all(serde_json::to_string(response)?.as_bytes())
        .await?;
    output.write_all(b"\n").await?;
    output.flush().await?;
    Ok(())
}

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

async fn read_message<R: tokio::io::AsyncBufRead + Unpin>(
    reader: &mut R,
) -> Result<Option<(String, bool)>> {
    let mut message = Vec::new();
    let mut too_large = false;
    loop {
        let bytes = reader.fill_buf().await?;
        if bytes.is_empty() {
            return if message.is_empty() && !too_large {
                Ok(None)
            } else {
                Ok(Some((
                    String::from_utf8_lossy(&message).into_owned(),
                    too_large,
                )))
            };
        }
        let end = bytes.iter().position(|b| *b == b'\n').map(|n| n + 1);
        let count = end.unwrap_or(bytes.len());
        if !too_large {
            if message.len() + count > 1024 * 1024 {
                too_large = true;
            } else {
                message.extend_from_slice(&bytes[..count]);
            }
        }
        reader.consume(count);
        if end.is_some() {
            return Ok(Some((
                String::from_utf8_lossy(&message).into_owned(),
                too_large,
            )));
        }
    }
}
