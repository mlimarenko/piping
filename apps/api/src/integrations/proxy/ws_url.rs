use reqwest::Url;
use uuid::Uuid;

use crate::contexts::connections::domain::ConnectionProtocol;
use crate::errors::AppError;

fn proxy_protocol_suffix(protocol: &ConnectionProtocol) -> Result<&'static str, AppError> {
    Ok(match protocol {
        ConnectionProtocol::Ssh => "terminal",
        ConnectionProtocol::Rdp => "rdp",
        ConnectionProtocol::Vnc => "vnc",
        ConnectionProtocol::App => {
            return Err(AppError::Validation(
                "APP protocol does not use proxy websocket path".to_string(),
            ));
        }
        ConnectionProtocol::Cctv => {
            return Err(AppError::Validation(
                "VIDEO protocol does not use proxy websocket path".to_string(),
            ));
        }
    })
}

fn client_protocol_suffix(protocol: &ConnectionProtocol) -> Result<&'static str, AppError> {
    match protocol {
        ConnectionProtocol::Ssh => Ok("ssh"),
        ConnectionProtocol::Rdp => Ok("rdp"),
        ConnectionProtocol::Vnc => Ok("vnc"),
        ConnectionProtocol::App => Err(AppError::Validation(
            "APP protocol does not use websocket path".to_string(),
        )),
        ConnectionProtocol::Cctv => Err(AppError::Validation(
            "VIDEO protocol does not use websocket path".to_string(),
        )),
    }
}

pub fn proxy_ws_path(protocol: &ConnectionProtocol, session_id: Uuid) -> Result<String, AppError> {
    let suffix = proxy_protocol_suffix(protocol)?;
    Ok(format!("/proxy/v1/ws/{}/{}", suffix, session_id))
}

pub fn client_ws_path(protocol: &ConnectionProtocol, session_id: Uuid) -> Result<String, AppError> {
    let suffix = client_protocol_suffix(protocol)?;
    Ok(format!("/api/v1/ws/{}/{}", suffix, session_id))
}

fn build_ws_url(base_url: &str, path: &str, token: Option<&str>) -> Result<String, AppError> {
    let mut url = Url::parse(base_url)
        .map_err(|error| AppError::Validation(format!("Invalid proxy base URL: {}", error)))?;
    let scheme = match url.scheme() {
        "https" => "wss",
        "http" => "ws",
        other => {
            return Err(AppError::Validation(format!(
                "Unsupported proxy URL scheme: {}",
                other
            )));
        }
    };
    url.set_scheme(scheme)
        .map_err(|_| AppError::Validation("Invalid proxy URL scheme".to_string()))?;
    let base_path = url.path().trim_end_matches('/');
    let normalized = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{}", path)
    };
    url.set_path(&format!("{}{}", base_path, normalized));
    url.set_query(None);
    if let Some(token) = token {
        url.query_pairs_mut().append_pair("token", token);
    }
    Ok(url.to_string())
}

pub fn build_proxy_ws_url(base_url: &str, path: &str, token: &str) -> Result<String, AppError> {
    build_ws_url(base_url, path, Some(token))
}

pub fn build_client_ws_url(base_url: &str, path: &str) -> Result<String, AppError> {
    build_ws_url(base_url, path, None)
}
