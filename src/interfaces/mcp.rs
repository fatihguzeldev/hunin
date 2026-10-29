use crate::operations::{Operations, catalog};
use anyhow::Result;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

// Includes handlers waiting to write their replies, independently of operation
// work admission and the existing per-message input byte limit.
const MAX_PENDING_RESPONSES: usize = 8;

pub async fn serve(operations: Operations) -> Result<()> {
    let result = serve_session(operations.clone()).await;
    let shutdown = operations.shutdown().await;
    result?;
    shutdown
}

async fn serve_session(operations: Operations) -> Result<()> {
    serve_io(
        operations,
        BufReader::new(tokio::io::stdin()),
        tokio::io::stdout(),
    )
    .await
}

async fn serve_io<R, W>(operations: Operations, mut input: R, output: W) -> Result<()>
where
    R: tokio::io::AsyncBufRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let output = std::sync::Arc::new(tokio::sync::Mutex::new(output));
    let mut initialized = false;
    let mut negotiated = false;
    let mut requests = tokio::task::JoinSet::new();
    let mut interrupted = false;
    loop {
        // Backpressure includes response delivery. A caller that stops reading
        // cannot accumulate an unbounded set of completed response values.
        if requests.len() >= MAX_PENDING_RESPONSES {
            let completed = tokio::select! {
                result = requests.join_next() => Some(result.unwrap()),
                _ = tokio::signal::ctrl_c() => {
                    interrupted = true;
                    requests.abort_all();
                    None
                },
            };
            let Some(completed) = completed else {
                break;
            };
            completed??;
        }
        let message = tokio::select! {
            message = read_message(&mut input) => message?,
            _ = tokio::signal::ctrl_c() => {
                interrupted = true;
                requests.abort_all();
                None
            },
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
        match result {
            Ok(result) => result?,
            Err(error) if interrupted && error.is_cancelled() => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

async fn write_response<W: tokio::io::AsyncWrite + Unpin>(
    output: &tokio::sync::Mutex<W>,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::Store;

    #[tokio::test]
    async fn reply_backpressure_bounds_requests_that_have_already_mutated_metadata() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::open(directory.path()).unwrap();
        let operations = Operations::new(store.clone());
        let (client, server) = tokio::io::duplex(64);
        let (read, write) = tokio::io::split(server);
        let serving = tokio::spawn(serve_io(operations, BufReader::new(read), write));
        let (read, mut write) = tokio::io::split(client);
        let mut read = BufReader::new(read);
        write.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2025-06-18\",\"capabilities\":{},\"clientInfo\":{}}}\n").await.unwrap();
        let mut initialized = String::new();
        read.read_line(&mut initialized).await.unwrap();
        assert!(
            serde_json::from_str::<Value>(&initialized)
                .unwrap()
                .get("result")
                .is_some()
        );
        write
            .write_all(b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}\n")
            .await
            .unwrap();
        let sending = tokio::spawn(async move {
            for index in 0..100 {
                let request = json!({"jsonrpc":"2.0","id":index+2,"method":"tools/call","params":{"name":"scenarios.save","arguments":{"id":format!("queued-{index}"),"name":"x".repeat(1024),"command":["unused"]}}});
                write
                    .write_all(format!("{request}\n").as_bytes())
                    .await
                    .unwrap();
            }
        });
        // Stop reading replies. Even fully executed mutations must continue
        // occupying a response slot until their bytes have been delivered.
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                if store.documents("scenarios").unwrap().len() >= MAX_PENDING_RESPONSES {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        for _ in 0..100 {
            tokio::task::yield_now().await;
        }
        assert_eq!(
            store.documents("scenarios").unwrap().len(),
            MAX_PENDING_RESPONSES
        );
        sending.abort();
        serving.abort();
        let _ = sending.await;
        let _ = serving.await;
    }
}
