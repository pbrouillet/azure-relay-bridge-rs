use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::StatusCode;
use tracing::{debug, warn};
use url::Url;

use crate::connection_string::RelayConnectionStringBuilder;
use crate::error::{RelayError, Result};
use crate::protocol::{actions, query_params, HC_PATH_PREFIX};
use crate::stream::HybridConnectionStream;
use crate::token_provider::{
    SecurityToken, SharedAccessSignatureToken, SharedAccessSignatureTokenProvider, TokenProvider,
};

/// Default operation timeout (70 seconds, matching C# SDK).
const DEFAULT_OPERATION_TIMEOUT: Duration = Duration::from_secs(70);

/// Default WebSocket keep-alive interval (3.5 minutes, matching C# SDK).
const DEFAULT_KEEP_ALIVE_INTERVAL: Duration = Duration::from_secs(210);

// ---------------------------------------------------------------------------
// Dyn-safe wrapper for TokenProvider (RPITIT traits are not dyn-compatible)
// ---------------------------------------------------------------------------

trait DynTokenProvider: Send + Sync {
    fn get_token_boxed<'a>(
        &'a self,
        audience: &'a str,
        valid_for: Duration,
    ) -> Pin<Box<dyn Future<Output = Result<SecurityToken>> + Send + 'a>>;
}

impl<T: TokenProvider> DynTokenProvider for T {
    fn get_token_boxed<'a>(
        &'a self,
        audience: &'a str,
        valid_for: Duration,
    ) -> Pin<Box<dyn Future<Output = Result<SecurityToken>> + Send + 'a>> {
        Box::pin(self.get_token(audience, valid_for))
    }
}

/// The sender-side client for Azure Relay Hybrid Connections.
///
/// Connects to an Azure Relay Hybrid Connection endpoint over WebSocket and
/// returns a [`HybridConnectionStream`] that implements `AsyncRead + AsyncWrite`.
pub struct HybridConnectionClient {
    /// The relay endpoint address (wss:// scheme with `$hc/` path prefix).
    address: Url,
    /// Token provider for authentication (SAS or AAD).
    token_provider: Option<Arc<dyn DynTokenProvider>>,
    /// Operation timeout for connection attempts.
    operation_timeout: Duration,
    /// WebSocket keep-alive interval.
    keep_alive_interval: Duration,
    /// Whether the token provider issues AAD Bearer tokens (vs SAS tokens).
    #[cfg(feature = "azure-identity")]
    is_aad: bool,
}

impl std::fmt::Debug for HybridConnectionClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut s = f.debug_struct("HybridConnectionClient");
        s.field("address", &self.address)
            .field("has_token_provider", &self.token_provider.is_some())
            .field("operation_timeout", &self.operation_timeout)
            .field("keep_alive_interval", &self.keep_alive_interval);
        #[cfg(feature = "azure-identity")]
        s.field("is_aad", &self.is_aad);
        s.finish()
    }
}

impl HybridConnectionClient {
    // -- Constructors --------------------------------------------------------

    /// Creates a client from a connection string.
    ///
    /// The connection string must contain an `Endpoint` and `EntityPath`.
    /// Authentication credentials are extracted from SAS fields when present.
    pub fn from_connection_string(connection_string: &str) -> Result<Self> {
        let builder = RelayConnectionStringBuilder::from_connection_string(connection_string)?;
        builder.validate()?;

        let endpoint = builder.endpoint().ok_or_else(|| {
            RelayError::InvalidConnectionString("missing Endpoint".into())
        })?;
        let entity_path = builder.entity_path().ok_or_else(|| {
            RelayError::InvalidConnectionString("missing EntityPath".into())
        })?;

        let address = build_relay_address(endpoint, entity_path)?;

        #[cfg(feature = "azure-identity")]
        let mut is_aad = false;

        let token_provider: Option<Arc<dyn DynTokenProvider>> =
            if let (Some(key_name), Some(key)) = (
                builder.shared_access_key_name(),
                builder.shared_access_key(),
            ) {
                Some(Arc::new(SharedAccessSignatureTokenProvider::new(
                    key_name, key,
                )?))
            } else if let Some(sig) = builder.shared_access_signature() {
                Some(Arc::new(SharedAccessSignatureToken::new(sig)?))
            } else {
                // No SAS credentials — fall back to AAD if the feature is enabled.
                #[cfg(feature = "azure-identity")]
                {
                    let provider = crate::aad_token_provider::AadTokenProvider::new()?;
                    is_aad = true;
                    Some(Arc::new(provider))
                }
                #[cfg(not(feature = "azure-identity"))]
                {
                    None
                }
            };

        let operation_timeout = builder
            .operation_timeout()
            .unwrap_or(DEFAULT_OPERATION_TIMEOUT);

        Ok(Self {
            address,
            token_provider,
            operation_timeout,
            keep_alive_interval: DEFAULT_KEEP_ALIVE_INTERVAL,
            #[cfg(feature = "azure-identity")]
            is_aad,
        })
    }

    /// Creates a client from a pre-built URI and token provider.
    pub fn from_uri(address: Url, token_provider: Arc<impl TokenProvider + 'static>) -> Self {
        Self {
            address,
            token_provider: Some(token_provider),
            operation_timeout: DEFAULT_OPERATION_TIMEOUT,
            keep_alive_interval: DEFAULT_KEEP_ALIVE_INTERVAL,
            #[cfg(feature = "azure-identity")]
            is_aad: false,
        }
    }

    /// Creates a client from a URI with no authentication.
    pub fn from_uri_no_auth(address: Url) -> Self {
        Self {
            address,
            token_provider: None,
            operation_timeout: DEFAULT_OPERATION_TIMEOUT,
            keep_alive_interval: DEFAULT_KEEP_ALIVE_INTERVAL,
            #[cfg(feature = "azure-identity")]
            is_aad: false,
        }
    }

    // -- Properties ----------------------------------------------------------

    /// Returns the relay endpoint address.
    pub fn address(&self) -> &Url {
        &self.address
    }

    /// Returns the operation timeout.
    pub fn operation_timeout(&self) -> Duration {
        self.operation_timeout
    }

    /// Sets the operation timeout.
    pub fn set_operation_timeout(&mut self, timeout: Duration) {
        self.operation_timeout = timeout;
    }

    /// Returns the WebSocket keep-alive interval.
    pub fn keep_alive_interval(&self) -> Duration {
        self.keep_alive_interval
    }

    /// Sets the WebSocket keep-alive interval.
    pub fn set_keep_alive_interval(&mut self, interval: Duration) {
        self.keep_alive_interval = interval;
    }

    // -- Connection ----------------------------------------------------------

    /// Opens a new Hybrid Connection stream with no custom headers.
    pub async fn create_connection(&self) -> Result<HybridConnectionStream> {
        self.create_connection_with_headers(&HashMap::new()).await
    }

    /// Opens a new Hybrid Connection stream, attaching the supplied HTTP headers
    /// to the WebSocket upgrade request.
    pub async fn create_connection_with_headers(
        &self,
        request_headers: &HashMap<String, String>,
    ) -> Result<HybridConnectionStream> {
        use crate::protocol::{headers, normalize_audience, RELAY_USER_AGENT_VALUE};

        let tracking_id = uuid::Uuid::new_v4().to_string();
        debug!(tracking_id = %tracking_id, "creating hybrid connection");

        // Build the WebSocket URL with protocol query parameters.
        let mut ws_url = self.address.clone();
        ws_url
            .query_pairs_mut()
            .append_pair(query_params::ACTION, actions::CONNECT)
            .append_pair(query_params::ID, &tracking_id);

        // Build the HTTP upgrade request.
        let mut request = ws_url
            .as_str()
            .into_client_request()
            .map_err(|e| RelayError::communication(format!("failed to build request: {e}")))?;

        // Add auth token — AAD tokens use standard `Authorization` header,
        // SAS tokens use the `ServiceBusAuthorization` header.
        if let Some(ref provider) = self.token_provider {
            let audience = normalize_audience(&self.address);
            let token = provider
                .get_token_boxed(&audience, Duration::from_secs(3600))
                .await?;

            #[cfg(feature = "azure-identity")]
            let header_name = if self.is_aad {
                "Authorization"
            } else {
                headers::SERVICE_BUS_AUTHORIZATION
            };
            #[cfg(not(feature = "azure-identity"))]
            let header_name = headers::SERVICE_BUS_AUTHORIZATION;

            request.headers_mut().insert(
                header_name,
                token.token.parse().map_err(|_| {
                    RelayError::communication("invalid token value for header")
                })?,
            );
        }

        // Add Relay-User-Agent header.
        request.headers_mut().insert(
            headers::RELAY_USER_AGENT,
            RELAY_USER_AGENT_VALUE.parse().unwrap(),
        );

        // Add custom request headers.
        for (key, value) in request_headers {
            let header_name: tokio_tungstenite::tungstenite::http::HeaderName =
                key.parse().map_err(|e: tokio_tungstenite::tungstenite::http::header::InvalidHeaderName| {
                    RelayError::InvalidArgument {
                        name: "request_headers".into(),
                        message: format!("invalid header name '{key}': {e}"),
                    }
                })?;
            let header_value: tokio_tungstenite::tungstenite::http::HeaderValue =
                value.parse().map_err(|e: tokio_tungstenite::tungstenite::http::header::InvalidHeaderValue| {
                    RelayError::InvalidArgument {
                        name: "request_headers".into(),
                        message: format!("invalid header value for '{key}': {e}"),
                    }
                })?;
            request.headers_mut().insert(header_name, header_value);
        }

        // Connect with a timeout.
        let connect_future = tokio_tungstenite::connect_async(request);
        let (ws_stream, _response) =
            tokio::time::timeout(self.operation_timeout, connect_future)
                .await
                .map_err(|_| {
                    warn!(tracking_id = %tracking_id, timeout = ?self.operation_timeout, "connection timed out");
                    RelayError::Timeout(self.operation_timeout)
                })?
                .map_err(|e| map_ws_error(e, &tracking_id))?;

        debug!(tracking_id = %tracking_id, "hybrid connection established");
        Ok(HybridConnectionStream::new(ws_stream, tracking_id))
    }
}

// -- Helpers -----------------------------------------------------------------

/// Converts an `sb://` endpoint and entity path to a `wss://…/$hc/{entity_path}` URL.
fn build_relay_address(endpoint: &Url, entity_path: &str) -> Result<Url> {
    let host = endpoint
        .host_str()
        .ok_or_else(|| RelayError::InvalidConnectionString("missing host".into()))?;
    let port = endpoint.port().unwrap_or(443);
    Url::parse(&format!(
        "wss://{}:{}/{}{}", host, port, HC_PATH_PREFIX, entity_path
    ))
    .map_err(|e| RelayError::InvalidConnectionString(e.to_string()))
}

/// Maps a WebSocket/tungstenite error to the appropriate `RelayError` variant.
fn map_ws_error(err: tokio_tungstenite::tungstenite::Error, tracking_id: &str) -> RelayError {
    use tokio_tungstenite::tungstenite::Error as WsError;
    match &err {
        WsError::Http(response) => {
            let status = response.status();
            let body_hint = response
                .body()
                .as_ref()
                .map(|b| String::from_utf8_lossy(b).to_string())
                .unwrap_or_default();
            match status {
                StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
                    RelayError::AuthorizationFailed(format!(
                        "server returned {status} for tracking_id={tracking_id}: {body_hint}"
                    ))
                }
                StatusCode::NOT_FOUND => RelayError::EndpointNotFound(format!(
                    "server returned 404 for tracking_id={tracking_id}: {body_hint}"
                )),
                _ => RelayError::Communication {
                    message: format!(
                        "server returned {status} for tracking_id={tracking_id}: {body_hint}"
                    ),
                    source: Some(Box::new(err)),
                },
            }
        }
        _ => RelayError::WebSocket(Box::new(err)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_audience_strips_hc_prefix_and_adds_trailing_slash() {
        use crate::protocol::normalize_audience;
        let url = Url::parse("wss://mynamespace.servicebus.windows.net/$hc/myconn").unwrap();
        assert_eq!(
            normalize_audience(&url),
            "http://mynamespace.servicebus.windows.net/myconn/"
        );
    }

    #[test]
    fn normalize_audience_lowercases_host() {
        use crate::protocol::normalize_audience;
        let url = Url::parse("wss://MyNamespace.ServiceBus.Windows.NET/$hc/myconn").unwrap();
        assert_eq!(
            normalize_audience(&url),
            "http://mynamespace.servicebus.windows.net/myconn/"
        );
    }

    #[test]
    fn normalize_audience_no_hc_prefix() {
        use crate::protocol::normalize_audience;
        let url = Url::parse("wss://host.example.com/some/path").unwrap();
        assert_eq!(normalize_audience(&url), "http://host.example.com/some/path/");
    }

    #[test]
    fn build_relay_address_converts_sb_to_wss() {
        let endpoint = Url::parse("sb://mynamespace.servicebus.windows.net/").unwrap();
        let addr = build_relay_address(&endpoint, "myconn").unwrap();
        assert_eq!(addr.scheme(), "wss");
        assert_eq!(
            addr.host_str().unwrap(),
            "mynamespace.servicebus.windows.net"
        );
        assert!(addr.path().contains("$hc/myconn"));
    }

    #[test]
    fn build_relay_address_missing_host() {
        // A URL like "sb:" has no host — should error.
        let endpoint = Url::parse("sb:///").unwrap();
        assert!(build_relay_address(&endpoint, "path").is_err());
    }

    #[test]
    fn from_connection_string_valid() {
        let cs = "Endpoint=sb://test.servicebus.windows.net;EntityPath=hc1;\
                   SharedAccessKeyName=key1;SharedAccessKey=c2VjcmV0";
        let client = HybridConnectionClient::from_connection_string(cs).unwrap();
        assert!(client.address().as_str().contains("$hc/hc1"));
        assert!(client.token_provider.is_some());
        assert_eq!(client.operation_timeout(), DEFAULT_OPERATION_TIMEOUT);
    }

    #[test]
    fn from_connection_string_missing_entity_path() {
        let cs = "Endpoint=sb://test.servicebus.windows.net;\
                   SharedAccessKeyName=key1;SharedAccessKey=c2VjcmV0";
        let err = HybridConnectionClient::from_connection_string(cs).unwrap_err();
        assert!(matches!(err, RelayError::InvalidConnectionString(_)));
    }

    #[test]
    fn from_connection_string_no_auth() {
        let cs = "Endpoint=sb://test.servicebus.windows.net;EntityPath=hc1";
        let client = HybridConnectionClient::from_connection_string(cs).unwrap();
        // With the azure-identity feature enabled, no-SAS falls back to AAD.
        #[cfg(feature = "azure-identity")]
        {
            assert!(client.token_provider.is_some());
            assert!(client.is_aad);
        }
        #[cfg(not(feature = "azure-identity"))]
        assert!(client.token_provider.is_none());
    }

    #[test]
    fn from_uri_no_auth_sets_defaults() {
        let url = Url::parse("wss://host.example.com/$hc/conn").unwrap();
        let client = HybridConnectionClient::from_uri_no_auth(url.clone());
        assert_eq!(client.address(), &url);
        assert!(client.token_provider.is_none());
        assert_eq!(client.operation_timeout(), DEFAULT_OPERATION_TIMEOUT);
        assert_eq!(client.keep_alive_interval(), DEFAULT_KEEP_ALIVE_INTERVAL);
    }

    #[test]
    fn setters_work() {
        let url = Url::parse("wss://host.example.com/$hc/conn").unwrap();
        let mut client = HybridConnectionClient::from_uri_no_auth(url);
        client.set_operation_timeout(Duration::from_secs(30));
        client.set_keep_alive_interval(Duration::from_secs(60));
        assert_eq!(client.operation_timeout(), Duration::from_secs(30));
        assert_eq!(client.keep_alive_interval(), Duration::from_secs(60));
    }

    #[test]
    fn from_uri_with_token_provider() {
        use crate::token_provider::SharedAccessSignatureTokenProvider;
        let url = Url::parse("wss://contoso.servicebus.windows.net/$hc/myconn").unwrap();
        let provider =
            Arc::new(SharedAccessSignatureTokenProvider::new("keyName", "keyValue").unwrap());
        let client = HybridConnectionClient::from_uri(url.clone(), provider);
        assert_eq!(client.address(), &url);
        assert_eq!(client.operation_timeout(), Duration::from_secs(70));
        assert_eq!(client.keep_alive_interval(), Duration::from_secs(210));
    }
}
