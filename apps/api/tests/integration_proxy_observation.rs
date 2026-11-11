//! Exercises the real proxy websocket admission boundary without contacting a target.

use std::{sync::Arc, time::Duration};

use axum::{
    body::to_bytes,
    http::{HeaderMap, HeaderValue, StatusCode},
    response::Response,
};
use piping_backend::config::ProxyConfig;
use piping_backend::integrations::proxy::{
    StaticProxySharedSecretProvider, create_proxy_observer_ws_token, create_proxy_ws_token,
};
use piping_backend::proxy::{ProxyState, build_proxy_router};
use piping_backend::proxy::{
    handle_create_connection, handle_delete_connection, handle_get_metrics,
};
use serde_json::{Value, json};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Error;
use uuid::Uuid;

async fn response_json(response: Response) -> Value {
    serde_json::from_slice(
        &to_bytes(response.into_body(), 131_072)
            .await
            .expect("bounded body"),
    )
    .expect("json")
}

#[tokio::test]
async fn proxy_generation_is_stable_until_end_and_new_on_recreation() {
    let secret = Uuid::new_v4().to_string();
    let node = "observation-generation-fixture";
    let provider = Arc::new(
        StaticProxySharedSecretProvider::new(node.to_string(), secret.clone()).expect("provider"),
    );
    let state =
        Arc::new(ProxyState::new_with_provider(&ProxyConfig::default(), provider).expect("state"));
    let mut headers = HeaderMap::new();
    headers.insert(
        "x-piping-proxy-node-id",
        HeaderValue::from_static("observation-generation-fixture"),
    );
    headers.insert(
        "x-piping-proxy-secret",
        HeaderValue::from_str(&secret).expect("credential"),
    );
    let id = Uuid::new_v4();
    let payload =
        json!({"connection_id": id, "connection_type": "ssh", "host": "127.0.0.1", "port": 1});
    let response = handle_create_connection(state.clone(), headers.clone(), payload.clone()).await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let created = response_json(response).await;
    let generation = created["generation"].as_str().expect("generation");
    let duplicate = handle_create_connection(state.clone(), headers.clone(), payload.clone()).await;
    assert_eq!(duplicate.status(), StatusCode::CONFLICT);
    let presence =
        response_json(handle_get_metrics(state.clone(), headers.clone(), id).await).await;
    assert_eq!(presence["runtime"]["generation"], generation);
    assert_eq!(presence["runtime"]["state"], "unknown");
    assert!(presence["runtime"]["ended_at"].is_null());

    let ended = handle_delete_connection(state.clone(), headers.clone(), id).await;
    assert_eq!(ended.status(), StatusCode::NO_CONTENT);
    let first = response_json(handle_get_metrics(state.clone(), headers.clone(), id).await).await;
    let second = response_json(handle_get_metrics(state.clone(), headers.clone(), id).await).await;
    assert_eq!(first["runtime"]["state"], "ended");
    assert_eq!(first["runtime"]["generation"], generation);
    assert_eq!(first["runtime"]["ended_at"], second["runtime"]["ended_at"]);
    assert!(first["runtime"]["ended_at"].as_str().is_some());
    let recreated =
        response_json(handle_create_connection(state.clone(), headers.clone(), payload).await)
            .await;
    assert_ne!(recreated["generation"], created["generation"]);
    let current = response_json(handle_get_metrics(state, headers, id).await).await;
    assert_eq!(current["runtime"]["generation"], recreated["generation"]);
    assert_ne!(current["runtime"]["state"], "ended");
}

#[tokio::test]
async fn observer_credential_cannot_enter_any_operator_websocket() {
    let secret = Uuid::new_v4().to_string();
    let provider = Arc::new(
        StaticProxySharedSecretProvider::new("observation-fixture".to_string(), secret.clone())
            .expect("provider"),
    );
    let state =
        Arc::new(ProxyState::new_with_provider(&ProxyConfig::default(), provider).expect("state"));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("address");
    let server =
        tokio::spawn(async move { axum::serve(listener, build_proxy_router(state)).await });
    let connection = Uuid::new_v4();
    let actor = Uuid::new_v4();
    let token =
        create_proxy_observer_ws_token(&secret, connection, actor, Uuid::new_v4()).expect("token");
    for protocol in ["terminal", "rdp", "vnc"] {
        let result = tokio::time::timeout(
            Duration::from_secs(30),
            connect_async(format!(
                "ws://{addr}/proxy/v1/ws/{protocol}/{connection}?token={token}"
            )),
        )
        .await
        .expect("websocket admission deadline");
        assert!(
            matches!(result, Err(Error::Http(ref response)) if response.status().as_u16() == 401)
        );
    }
    let operator_token = create_proxy_ws_token(&secret, connection, actor).expect("token");
    let result = tokio::time::timeout(
        Duration::from_secs(30),
        connect_async(format!(
            "ws://{addr}/proxy/v1/ws/observe/{connection}?token={operator_token}"
        )),
    )
    .await
    .expect("websocket admission deadline");
    assert!(matches!(result, Err(Error::Http(ref response)) if response.status().as_u16() == 401));
    server.abort();
}
