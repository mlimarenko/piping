//! Proxy integration barrel: proxy and HTTP clients, metrics, shared-secret provider and WebSocket token/URL helpers.
pub mod client;
pub mod http_client;
pub mod metrics;
pub mod shared_secret_provider;
pub mod ws_token;
pub mod ws_url;

pub use client::{ProxyClient, ProxySession};
pub use http_client::HttpProxyClient;
pub use metrics::{ProxyLinkMetricsResponse, ProxyMetricsResponse, ProxyMetricsSample};
pub use shared_secret_provider::{
    EnvProxySharedSecretProvider, PROXY_NODE_ID_ENV_KEY, PROXY_NODE_SECRET_ENV_KEY,
    ProxyCredential, ProxySharedSecretProvider, StaticProxySharedSecretProvider,
    generate_proxy_secret_material,
};
pub use ws_token::{
    ProxyWsTokenPayload, create_proxy_ws_token, validate_proxy_ws_token,
    validate_proxy_ws_token_from_provider,
};
pub use ws_url::{build_client_ws_url, build_proxy_ws_url, client_ws_path, proxy_ws_path};
