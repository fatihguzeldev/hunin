use crate::operations::Operations;
use anyhow::Result;
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{net::SocketAddr, path::PathBuf};
use tower_http::services::ServeDir;

#[derive(Clone)]
struct HttpState {
    operations: Operations,
    host: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    operation: String,
    #[serde(default = "empty")]
    params: Value,
}
fn empty() -> Value {
    json!({})
}

pub async fn serve(operations: Operations, address: SocketAddr, ui: PathBuf) -> Result<()> {
    anyhow::ensure!(
        address.ip().is_loopback(),
        "Hunin's local UI must bind to a loopback address"
    );
    let listener = tokio::net::TcpListener::bind(address).await?;
    let local = listener.local_addr()?;
    let shutdown = operations.clone();
    let state = HttpState {
        operations,
        host: local.to_string(),
    };
    let router = Router::new()
        .route("/health", get(|| async { Json(json!({"status":"ok"})) }))
        .route("/api/operations", post(call))
        .fallback_service(ServeDir::new(ui).append_index_html_on_directories(true))
        .layer(DefaultBodyLimit::max(1024 * 1024))
        .layer(middleware::from_fn(headers))
        .with_state(state);
    eprintln!("Hunin UI: http://{local}");
    let result = axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            let _ = tokio::signal::ctrl_c().await;
            if let Err(error) = shutdown.shutdown().await {
                eprintln!("Hunin shutdown failed: {error:#}");
            }
        })
        .await;
    result?;
    Ok(())
}

async fn call(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Json(request): Json<Request>,
) -> Response {
    let host = headers.get(header::HOST).and_then(|v| v.to_str().ok());
    let origin = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok());
    if host != Some(state.host.as_str())
        || origin.is_some_and(|s| s != format!("http://{}", state.host))
    {
        return (
            StatusCode::FORBIDDEN,
            Json(
                json!({"error":{"code":"forbidden_origin","message":"Use the local Hunin origin"}}),
            ),
        )
            .into_response();
    }
    match state
        .operations
        .call(&request.operation, request.params)
        .await
    {
        Ok(result) => Json(json!({"result":result})).into_response(),
        Err(error) => (
            StatusCode::BAD_REQUEST,
            Json(json!({"error":{"code":"operation_failed","message":format!("{error:#}"),"details":{"operation_error":crate::operations::operation_failure(&request.operation, &error)}}})),
        )
            .into_response(),
    }
}

async fn headers(request: axum::extract::Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    for (name, value) in [
        ("x-content-type-options", "nosniff"),
        ("x-frame-options", "DENY"),
        ("referrer-policy", "no-referrer"),
        (
            "content-security-policy",
            "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; connect-src 'self'; img-src 'self' data:; frame-ancestors 'none'; base-uri 'none'",
        ),
    ] {
        response.headers_mut().insert(
            axum::http::HeaderName::from_static(name),
            axum::http::HeaderValue::from_static(value),
        );
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{model::runs::Run, storage::Store};

    #[tokio::test]
    async fn operation_failures_have_the_same_typed_origin_in_http_and_mcp() {
        let data = tempfile::tempdir().unwrap();
        let store = Store::open(data.path()).unwrap();
        store
            .save_run(&Run {
                id: "recording".into(),
                name: "error control".into(),
                status: "completed".into(),
                command: vec![],
                cwd: ".".into(),
                runtime: "none".into(),
                started_at_us: 1,
                finished_at_us: Some(2),
                exit_code: Some(0),
                diagnostics: vec![],
                source: json!({}),
                correctness: json!({}),
            })
            .unwrap();
        store
            .save_document(
                "analysis-results",
                "active",
                &json!({"run_id":"recording","status":"running"}),
            )
            .unwrap();
        let operations = Operations::new(store);
        for (name, params, code) in [
            ("not-an-operation", json!({}), "unknown_operation"),
            ("runs.get", json!({}), "invalid_parameters"),
            ("runs.remove", json!({"run_id":"recording"}), "conflict"),
            (
                "runs.get",
                json!({"run_id":"unknown operation invalid parameters conflict"}),
                "operation_failed",
            ),
        ] {
            let rpc = crate::interfaces::mcp::handle(&operations, &json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":name,"arguments":params}}), true).await;
            let host = "127.0.0.1:17879";
            let mut headers = HeaderMap::new();
            headers.insert(header::HOST, host.parse().unwrap());
            headers.insert(header::ORIGIN, format!("http://{host}").parse().unwrap());
            let response = call(
                State(HttpState {
                    operations: operations.clone(),
                    host: host.into(),
                }),
                headers,
                Json(Request {
                    operation: name.into(),
                    params,
                }),
            )
            .await;
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
            let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
                .await
                .unwrap();
            let http: Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(http["error"]["code"], "operation_failed");
            assert_eq!(rpc["result"]["isError"], true);
            assert!(rpc["result"].get("structuredContent").is_none());
            assert_eq!(
                http["error"]["message"],
                rpc["result"]["content"][0]["text"]
            );
            let failure = &http["error"]["details"]["operation_error"];
            assert_eq!(failure["code"], code, "{http}");
            assert_eq!(failure["operation"], name);
            assert_eq!(failure, &rpc["result"]["_meta"]["hunin/operation-error"]);
            assert_eq!(failure["message"], http["error"]["message"]);
            let definitions: Value =
                serde_json::from_str(include_str!("../../protocol/response.schema.json")).unwrap();
            let schema = &definitions["$defs"]["OperationFailure"];
            jsonschema::validator_for(schema)
                .unwrap()
                .validate(failure)
                .unwrap();
        }
    }
}
