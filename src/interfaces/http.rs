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
            Json(json!({"error":{"code":"operation_failed","message":format!("{error:#}")}})),
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
