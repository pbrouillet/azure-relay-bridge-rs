use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use url::Url;

/// Remote endpoint information for a connection.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RemoteEndpoint {
    pub address: String,
    pub port: u16,
}

/// Accept command sent by the service to the listener when a sender connects.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AcceptCommand {
    /// The rendezvous WebSocket URL to connect to for accepting the connection.
    pub address: String,
    /// Unique connection identifier (GUID).
    pub id: String,
    /// HTTP headers from the sender's WebSocket upgrade request.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub connect_headers: HashMap<String, String>,
    /// The sender's remote endpoint (IP + port).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_endpoint: Option<RemoteEndpoint>,
}

/// HTTP request command sent by the service to the listener.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RequestCommand {
    /// Rendezvous address for large request/response upgrade.
    pub address: String,
    /// Unique request identifier.
    pub id: String,
    /// The HTTP request target (path + query string).
    pub request_target: String,
    /// HTTP method (GET, POST, etc.).
    pub method: String,
    /// HTTP request headers.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub request_headers: HashMap<String, String>,
    /// Whether the request has a body (binary frames follow).
    pub body: bool,
    /// The sender's remote endpoint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_endpoint: Option<RemoteEndpoint>,
}

/// HTTP response command sent by the listener back to the service.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ResponseCommand {
    /// The request ID this response is for.
    pub request_id: String,
    /// HTTP status code.
    pub status_code: u16,
    /// Optional HTTP status description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_description: Option<String>,
    /// HTTP response headers.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub response_headers: HashMap<String, String>,
    /// Whether the response has a body.
    pub body: bool,
}

/// Token renewal command sent by the listener to the service.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RenewTokenCommand {
    pub token: String,
}

/// Envelope for commands received by the listener from the service.
/// The JSON has one top-level key: "accept" or "request".
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum ListenerCommand {
    Accept(AcceptCommand),
    Request(RequestCommand),
}

/// Envelope for commands sent by the listener to the service.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum ListenerResponse {
    Response(ResponseCommand),
    RenewToken(RenewTokenCommand),
}

// ---------------------------------------------------------------------------
// URI construction utilities
// ---------------------------------------------------------------------------

/// The WebSocket path prefix for Hybrid Connections.
pub const HC_PATH_PREFIX: &str = "$hc/";

/// Query parameter names used by the Hybrid Connections protocol.
pub mod query_params {
    pub const ACTION: &str = "sb-hc-action";
    pub const ID: &str = "sb-hc-id";
    pub const TOKEN: &str = "sb-hc-token";
    pub const STATUS_CODE: &str = "sb-hc-statusCode";
    pub const STATUS_DESCRIPTION: &str = "sb-hc-statusDescription";
}

/// Actions for the `sb-hc-action` query parameter.
pub mod actions {
    pub const LISTEN: &str = "listen";
    pub const CONNECT: &str = "connect";
    pub const ACCEPT: &str = "accept";
    pub const REQUEST: &str = "request";
}

/// HTTP header names used by the Azure Relay protocol.
pub mod headers {
    /// The authentication header for Azure Relay (NOT the standard `Authorization` header).
    pub const SERVICE_BUS_AUTHORIZATION: &str = "ServiceBusAuthorization";
    /// Custom user-agent header for Azure Relay (NOT the standard `User-Agent` header).
    pub const RELAY_USER_AGENT: &str = "Relay-User-Agent";
}

/// The user-agent value sent with all relay connections.
pub const RELAY_USER_AGENT_VALUE: &str = concat!("azure-relay-rs/", env!("CARGO_PKG_VERSION"));

/// Normalizes a relay address into an audience URL for SAS token generation.
///
/// Converts `sb://` or `wss://` scheme to `http://`, lowercases the host,
/// strips the `/$hc/` path prefix, and ensures a trailing `/`.
/// This matches the C# SDK's `SharedAccessSignatureTokenProvider` behavior.
///
/// # Examples
/// - `wss://contoso.servicebus.windows.net/$hc/myconn` → `http://contoso.servicebus.windows.net/myconn/`
/// - `sb://CONTOSO.servicebus.windows.net/myconn` → `http://contoso.servicebus.windows.net/myconn/`
pub fn normalize_audience(address: &url::Url) -> String {
    let host = address.host_str().unwrap_or_default().to_lowercase();
    let path = address.path();
    // Strip /$hc/ prefix if present
    let clean_path = path
        .strip_prefix("/$hc/")
        .or_else(|| path.strip_prefix("/$hc"))
        .unwrap_or(path);
    let clean_path = clean_path.strip_prefix('/').unwrap_or(clean_path);
    // Build http:// URL with trailing slash
    let mut audience = format!("http://{}/{}", host, clean_path);
    if !audience.ends_with('/') {
        audience.push('/');
    }
    audience
}

/// Builds a Hybrid Connection WebSocket URI.
///
/// Format: `wss://{host}:{port}/$hc/{path}?sb-hc-action={action}&sb-hc-id={id}`
pub fn build_uri(host: &str, port: u16, path: &str, action: &str, id: &str) -> Url {
    build_uri_scheme("wss", host, port, path, action, id)
}

/// Builds a Hybrid Connection WebSocket URI with a specified scheme.
pub fn build_uri_scheme(
    scheme: &str,
    host: &str,
    port: u16,
    path: &str,
    action: &str,
    id: &str,
) -> Url {
    let mut url = Url::parse(&format!("{}://{}:{}/$hc/{}", scheme, host, port, path))
        .expect("valid URL components");
    url.query_pairs_mut()
        .append_pair(query_params::ACTION, action)
        .append_pair(query_params::ID, id);
    url
}

/// Builds a Hybrid Connection WebSocket URI with a SAS token.
pub fn build_uri_with_token(
    host: &str,
    port: u16,
    path: &str,
    action: &str,
    id: &str,
    token: &str,
) -> Url {
    let mut url = build_uri(host, port, path, action, id);
    url.query_pairs_mut()
        .append_pair(query_params::TOKEN, token);
    url
}

/// Filters out `sb-hc-*` query parameters from a URL, preserving all others.
pub fn filter_hybrid_connection_query_params(uri: &Url) -> Url {
    let mut filtered = uri.clone();
    let pairs: Vec<(String, String)> = filtered
        .query_pairs()
        .filter(|(key, _)| !key.starts_with("sb-hc-"))
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    filtered.set_query(None);
    if !pairs.is_empty() {
        let mut query = filtered.query_pairs_mut();
        for (k, v) in &pairs {
            query.append_pair(k, v);
        }
    }
    filtered
}
