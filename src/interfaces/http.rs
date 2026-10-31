use super::projects::ProjectSessions;
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
    sessions: ProjectSessions,
    address: SocketAddr,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    #[serde(default)]
    project_id: Option<String>,
    operation: String,
    #[serde(default = "empty")]
    params: Value,
}
fn empty() -> Value {
    json!({})
}

pub async fn serve(operations: Operations, address: SocketAddr, ui: PathBuf) -> Result<()> {
    serve_projects(ProjectSessions::direct(operations), address, ui).await
}

pub async fn serve_projects(
    sessions: ProjectSessions,
    address: SocketAddr,
    ui: PathBuf,
) -> Result<()> {
    anyhow::ensure!(
        address.ip().is_loopback(),
        "Hunin's local UI must bind to a loopback address"
    );
    let listener = tokio::net::TcpListener::bind(address).await?;
    let local = listener.local_addr()?;
    let shutdown = sessions.clone();
    let state = HttpState {
        sessions: sessions.clone(),
        address: local,
    };
    let router = Router::new()
        .route("/health", get(|| async { Json(json!({"status":"ok"})) }))
        .route("/api/operations", post(call))
        .route("/api/projects", get(projects))
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
    if result.is_err() {
        sessions.shutdown().await?;
    }
    result?;
    Ok(())
}

async fn call(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Json(request): Json<Request>,
) -> Response {
    if !local_origin(&state, &headers) {
        return forbidden_origin();
    }
    let operations = match state
        .sessions
        .operations(request.project_id.as_deref())
        .await
    {
        Ok(operations) => operations,
        Err(error) => return (
            StatusCode::BAD_REQUEST,
            Json(
                json!({"error":{"code":"project_selection_failed","message":format!("{error:#}")}}),
            ),
        )
            .into_response(),
    };
    match operations.call(&request.operation, request.params).await {
        Ok(result) => Json(json!({"result":result})).into_response(),
        Err(error) => (
            StatusCode::BAD_REQUEST,
            Json(json!({"error":{"code":"operation_failed","message":format!("{error:#}"),"details":{"operation_error":crate::operations::operation_failure(&request.operation, &error)}}})),
        )
            .into_response(),
    }
}

async fn projects(State(state): State<HttpState>, headers: HeaderMap) -> Response {
    if !local_origin(&state, &headers) {
        return forbidden_origin();
    }
    match state.sessions.catalog().await {
        Ok(catalog) => Json(catalog).into_response(),
        Err(error) => (
            StatusCode::BAD_REQUEST,
            Json(json!({"error":{"code":"project_lookup_failed","message":format!("{error:#}")}})),
        )
            .into_response(),
    }
}

fn local_origin(state: &HttpState, headers: &HeaderMap) -> bool {
    let Some(host) = headers.get(header::HOST).and_then(|v| v.to_str().ok()) else {
        return false;
    };
    if host != state.address.to_string() && host != format!("localhost:{}", state.address.port()) {
        return false;
    }
    match headers.get(header::ORIGIN) {
        None => true,
        Some(origin) => origin
            .to_str()
            .is_ok_and(|origin| origin == format!("http://{host}")),
    }
}

fn forbidden_origin() -> Response {
    (
        StatusCode::FORBIDDEN,
        Json(json!({"error":{"code":"forbidden_origin","message":"Use the local Hunin origin"}})),
    )
        .into_response()
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

    fn local_headers(host: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, host.parse().unwrap());
        headers.insert(header::ORIGIN, format!("http://{host}").parse().unwrap());
        headers
    }

    #[tokio::test]
    async fn localhost_can_load_projects_and_call_operations() {
        let root = tempfile::tempdir().unwrap();
        let registry = crate::projects::ProjectRegistry::open(root.path()).unwrap();
        let project = registry.create("project", None).unwrap();
        let state = HttpState {
            sessions: ProjectSessions::registry(registry, None),
            address: "127.0.0.1:17879".parse().unwrap(),
        };
        let response = projects(State(state.clone()), local_headers("localhost:17879")).await;
        assert_eq!(response.status(), StatusCode::OK);
        let response = call(
            State(state.clone()),
            local_headers("localhost:17879"),
            Json(Request {
                project_id: Some(project.id),
                operation: "runs.list".into(),
                params: json!({}),
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        state.sessions.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn local_origin_requires_the_bound_port_and_same_request_origin() {
        for address in ["127.0.0.1:17879", "[::1]:17879"] {
            let root = tempfile::tempdir().unwrap();
            let state = HttpState {
                sessions: ProjectSessions::direct(Operations::new(
                    Store::open(root.path()).unwrap(),
                )),
                address: address.parse().unwrap(),
            };
            for host in [address, "localhost:17879"] {
                let mut request = local_headers(host);
                assert!(local_origin(&state, &request), "{address}: {host}");
                request.remove(header::ORIGIN);
                assert!(local_origin(&state, &request));
                for origin in [
                    "http://attacker.example:17879",
                    "http://localhost:17880",
                    "https://localhost:17879",
                    "http://localhost:17879.attacker.example",
                    "http://localhost:17879/path",
                    "null",
                ] {
                    request.insert(header::ORIGIN, origin.parse().unwrap());
                    assert!(!local_origin(&state, &request), "{host}: {origin}");
                }
                let other_host = if host == address {
                    "localhost:17879"
                } else {
                    address
                };
                request.insert(
                    header::ORIGIN,
                    format!("http://{other_host}").parse().unwrap(),
                );
                assert!(!local_origin(&state, &request));
                request.insert(
                    header::ORIGIN,
                    axum::http::HeaderValue::from_bytes(b"\xff").unwrap(),
                );
                assert!(!local_origin(&state, &request));
            }
            for host in [
                "attacker.example:17879",
                "localhost.attacker.example:17879",
                "localhost:17880",
                "127.0.0.1:17880",
                "[::1]:17880",
                "0.0.0.0:17879",
            ] {
                assert!(!local_origin(&state, &local_headers(host)), "{host}");
            }
            assert!(!local_origin(&state, &HeaderMap::new()));
            state.sessions.shutdown().await.unwrap();
        }
    }

    #[tokio::test]
    async fn project_catalog_checks_origin_and_registry_operations_require_a_stable_id() {
        let root = tempfile::tempdir().unwrap();
        let registry = crate::projects::ProjectRegistry::open(root.path()).unwrap();
        let project = registry.create("project", None).unwrap();
        let host = "127.0.0.1:17879";
        let state = HttpState {
            sessions: ProjectSessions::registry(registry, Some(project.id.clone())),
            address: host.parse().unwrap(),
        };
        let response = projects(State(state.clone()), HeaderMap::new()).await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let response = projects(State(state.clone()), local_headers(host)).await;
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        let catalog: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(catalog["selected_project_id"], project.id);
        assert_eq!(catalog["projects"][0]["id"], project.id);
        for id in [None, Some("project".into()), Some("missing".into())] {
            let response = call(
                State(state.clone()),
                local_headers(host),
                Json(Request {
                    project_id: id,
                    operation: "runs.list".into(),
                    params: json!({}),
                }),
            )
            .await;
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
            let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
                .await
                .unwrap();
            let failure: Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(failure["error"]["code"], "project_selection_failed");
        }
        let response = call(
            State(state.clone()),
            local_headers(host),
            Json(Request {
                project_id: Some(project.id),
                operation: "runs.list".into(),
                params: json!({}),
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        state.sessions.shutdown().await.unwrap();
    }

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
                    sessions: ProjectSessions::direct(operations.clone()),
                    address: host.parse().unwrap(),
                }),
                headers,
                Json(Request {
                    project_id: None,
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
