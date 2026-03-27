use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, Notify, RwLock};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use url::Url;

use crate::connection_string::RelayConnectionStringBuilder;
use crate::error::{RelayError, Result};
use crate::http::RequestHandler;
use crate::protocol::{self, AcceptCommand, ListenerCommand, ListenerResponse, RenewTokenCommand};
use crate::stream::HybridConnectionStream;
use crate::token_provider::{
    SecurityToken, SharedAccessSignatureToken, SharedAccessSignatureTokenProvider, TokenProvider,
};

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

const DEFAULT_KEEP_ALIVE_INTERVAL: Duration = Duration::from_secs(210); // 3.5 min
const TOKEN_RENEWAL_MARGIN: Duration = Duration::from_secs(300); // 5 min before expiry
const ACCEPT_CHANNEL_CAPACITY: usize = 100;

/// Reconnect backoff sequence (matching C# SDK).
const RECONNECT_DELAYS: &[Duration] = &[
    Duration::from_secs(0),
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(5),
    Duration::from_secs(10),
    Duration::from_secs(30),
];

/// Connection status events.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionStatus {
    Connecting,
    Online,
    Offline,
}

/// Listens for incoming Azure Relay Hybrid Connections.
///
/// Opens a control-channel WebSocket to the relay service, receives `accept`
/// commands for new sender connections, and opens rendezvous WebSockets that
/// are yielded via [`accept_connection`](Self::accept_connection).
#[derive(Clone)]
pub struct HybridConnectionListener {
    inner: Arc<ListenerInner>,
}

impl std::fmt::Debug for HybridConnectionListener {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HybridConnectionListener")
            .field("address", &self.inner.address)
            .field("has_token_provider", &self.inner.token_provider.is_some())
            .field("is_open", &self.inner.is_open.load(Ordering::Relaxed))
            .finish()
    }
}

struct ListenerInner {
    /// The relay endpoint address (wss://host/$hc/path).
    address: Url,
    /// Token provider for authentication.
    token_provider: Option<Arc<dyn DynTokenProvider>>,
    /// Keep-alive interval for control channel pings.
    keep_alive_interval: Duration,
    /// Channel sender for accepted connections.
    accept_tx: mpsc::Sender<HybridConnectionStream>,
    /// Channel receiver for accepted connections.
    accept_rx: tokio::sync::Mutex<mpsc::Receiver<HybridConnectionStream>>,
    /// Current connection status.
    status: RwLock<ConnectionStatus>,
    /// Status change notification.
    status_notify: Notify,
    /// Shutdown signal.
    shutdown: Notify,
    /// Whether the listener is open.
    is_open: AtomicBool,
    /// The last error encountered.
    last_error: RwLock<Option<RelayError>>,
    /// Optional handler for HTTP request commands.
    request_handler: RwLock<Option<Arc<dyn crate::http::DynRequestHandler>>>,
    /// Whether the token provider issues AAD Bearer tokens (vs SAS tokens).
    #[cfg(feature = "azure-identity")]
    is_aad: bool,
}

impl HybridConnectionListener {
    // ------------------------------------------------------------------
    // Constructors
    // ------------------------------------------------------------------

    /// Creates a listener from an Azure Relay connection string.
    ///
    /// The connection string must contain `Endpoint` and `EntityPath`, plus
    /// either `SharedAccessKeyName`/`SharedAccessKey` or
    /// `SharedAccessSignature` for authentication. When no SAS credentials
    /// are provided and the `azure-identity` feature is enabled, AAD
    /// authentication via `DeveloperToolsCredential` is used automatically.
    pub fn from_connection_string(connection_string: &str) -> Result<Self> {
        let builder = RelayConnectionStringBuilder::from_connection_string(connection_string)?;

        let endpoint = builder.endpoint().ok_or_else(|| {
            RelayError::InvalidConnectionString("missing Endpoint".to_string())
        })?;
        let entity_path = builder.entity_path().ok_or_else(|| {
            RelayError::InvalidConnectionString("missing EntityPath".to_string())
        })?;

        let host = endpoint.host_str().ok_or_else(|| {
            RelayError::InvalidConnectionString("Endpoint has no host".to_string())
        })?;

        let address = Url::parse(&format!("wss://{}/{}{}", host, protocol::HC_PATH_PREFIX, entity_path))
            .map_err(|e| RelayError::InvalidConnectionString(format!("failed to build relay URL: {e}")))?;

        #[cfg(feature = "azure-identity")]
        let mut is_aad = false;

        // Build a token provider from connection string credentials.
        let token_provider: Option<Arc<dyn DynTokenProvider>> =
            if let (Some(key_name), Some(key)) = (
                builder.shared_access_key_name(),
                builder.shared_access_key(),
            ) {
                Some(Arc::new(SharedAccessSignatureTokenProvider::new(key_name, key)?))
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

        Ok(Self::new_inner(
            address,
            token_provider,
            #[cfg(feature = "azure-identity")]
            is_aad,
        ))
    }

    /// Creates a listener from a relay address and token provider.
    pub fn from_uri(address: Url, token_provider: Arc<impl TokenProvider + 'static>) -> Self {
        Self::new_inner(
            address,
            Some(token_provider),
            #[cfg(feature = "azure-identity")]
            false,
        )
    }

    /// Creates a listener from a relay address with no authentication.
    pub fn from_uri_no_auth(address: Url) -> Self {
        Self::new_inner(
            address,
            None,
            #[cfg(feature = "azure-identity")]
            false,
        )
    }

    fn new_inner(
        address: Url,
        token_provider: Option<Arc<dyn DynTokenProvider>>,
        #[cfg(feature = "azure-identity")] is_aad: bool,
    ) -> Self {
        let (accept_tx, accept_rx) = mpsc::channel(ACCEPT_CHANNEL_CAPACITY);

        let inner = Arc::new(ListenerInner {
            address,
            token_provider,
            keep_alive_interval: DEFAULT_KEEP_ALIVE_INTERVAL,
            accept_tx,
            accept_rx: tokio::sync::Mutex::new(accept_rx),
            status: RwLock::new(ConnectionStatus::Offline),
            status_notify: Notify::new(),
            shutdown: Notify::new(),
            is_open: AtomicBool::new(false),
            last_error: RwLock::new(None),
            request_handler: RwLock::new(None),
            #[cfg(feature = "azure-identity")]
            is_aad,
        });

        Self { inner }
    }

    // ------------------------------------------------------------------
    // Public API
    // ------------------------------------------------------------------

    /// Sets the handler for HTTP requests received via the relay.
    ///
    /// When set, incoming HTTP `request` commands on the control channel
    /// will be dispatched to this handler instead of being ignored.
    pub fn set_request_handler(&self, handler: impl RequestHandler) {
        let handler: Arc<dyn crate::http::DynRequestHandler> = Arc::new(handler);
        // Use try_write to avoid blocking; this is only called during setup.
        if let Ok(mut guard) = self.inner.request_handler.try_write() {
            *guard = Some(handler);
        } else {
            // Fallback: block.
            let rt = tokio::runtime::Handle::current();
            rt.block_on(async {
                *self.inner.request_handler.write().await = Some(handler);
            });
        }
    }

    /// Opens the listener — starts the control channel connection loop.
    ///
    /// Spawns a background task that maintains the control channel and
    /// reconnects with exponential backoff on failures.
    pub async fn open(&self) -> Result<()> {
        self.inner.is_open.store(true, Ordering::Release);
        *self.inner.status.write().await = ConnectionStatus::Connecting;
        self.inner.status_notify.notify_waiters();

        let inner = self.inner.clone();
        tokio::spawn(async move {
            Self::control_loop(inner).await;
        });

        Ok(())
    }

    /// Closes the listener — stops accepting new connections.
    pub async fn close(&self) -> Result<()> {
        self.inner.is_open.store(false, Ordering::Release);
        self.inner.shutdown.notify_waiters();
        *self.inner.status.write().await = ConnectionStatus::Offline;
        self.inner.status_notify.notify_waiters();
        Ok(())
    }

    /// Accepts the next incoming connection.
    ///
    /// Returns `Ok(None)` when the listener has been closed and no more
    /// connections will arrive.
    pub async fn accept_connection(&self) -> Result<Option<HybridConnectionStream>> {
        let mut rx = self.inner.accept_rx.lock().await;
        Ok(rx.recv().await)
    }

    /// Returns whether the listener is currently online (control channel connected).
    pub fn is_online(&self) -> bool {
        // Cheap non-blocking check via the atomic flag + a try_read.
        self.inner.is_open.load(Ordering::Acquire)
    }

    /// Returns the current connection status.
    pub fn status(&self) -> ConnectionStatus {
        // Use try_read to avoid blocking; fallback to Offline.
        self.inner
            .status
            .try_read()
            .map(|s| *s)
            .unwrap_or(ConnectionStatus::Offline)
    }

    /// Returns a human-readable description of the last error, if any.
    pub async fn last_error(&self) -> Option<String> {
        self.inner.last_error.read().await.as_ref().map(|e| e.to_string())
    }

    // ------------------------------------------------------------------
    // Control loop
    // ------------------------------------------------------------------

    async fn control_loop(inner: Arc<ListenerInner>) {
        let mut reconnect_index: usize = 0;

        loop {
            if !inner.is_open.load(Ordering::Acquire) {
                break;
            }

            // Set status to Connecting.
            {
                *inner.status.write().await = ConnectionStatus::Connecting;
                inner.status_notify.notify_waiters();
            }

            match Self::connect_control_channel(&inner).await {
                Ok(ws) => {
                    reconnect_index = 0; // Reset backoff on success.
                    {
                        *inner.status.write().await = ConnectionStatus::Online;
                        inner.status_notify.notify_waiters();
                    }

                    if let Err(e) = Self::run_control_channel(&inner, ws).await {
                        tracing::warn!("Control channel error: {}", e);
                        *inner.last_error.write().await = Some(e);
                    }
                }
                Err(e) => {
                    tracing::warn!("Control channel connection failed: {}", e);
                    *inner.last_error.write().await = Some(e);
                }
            }

            // Offline after disconnect / error.
            {
                *inner.status.write().await = ConnectionStatus::Offline;
                inner.status_notify.notify_waiters();
            }

            if !inner.is_open.load(Ordering::Acquire) {
                break;
            }

            // Backoff delay before reconnecting.
            let delay = RECONNECT_DELAYS[reconnect_index.min(RECONNECT_DELAYS.len() - 1)];
            reconnect_index += 1;

            tokio::select! {
                _ = tokio::time::sleep(delay) => {}
                _ = inner.shutdown.notified() => break,
            }
        }
    }

    // ------------------------------------------------------------------
    // Control channel connection
    // ------------------------------------------------------------------

    async fn connect_control_channel(
        inner: &Arc<ListenerInner>,
    ) -> Result<WebSocketStream<MaybeTlsStream<TcpStream>>> {
        use crate::protocol::{headers, normalize_audience, RELAY_USER_AGENT_VALUE};
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;

        let tracking_id = uuid::Uuid::new_v4().to_string();
        let host = inner.address.host_str().unwrap_or_default();
        let port = inner.address.port().unwrap_or(443);
        // The path after `/$hc/` in the address URL.
        let full_path = inner.address.path();
        let path = full_path
            .strip_prefix("/")
            .unwrap_or(full_path)
            .strip_prefix(protocol::HC_PATH_PREFIX)
            .unwrap_or(full_path);

        // Build the URL without the token (token goes in header, not query param).
        // Preserve the address scheme (wss:// in production, ws:// for mock testing).
        let scheme = match inner.address.scheme() {
            "ws" | "http" => "ws",
            _ => "wss",
        };
        let url = protocol::build_uri_scheme(scheme, host, port, path, protocol::actions::LISTEN, &tracking_id);

        // Build the HTTP upgrade request with auth header.
        let mut request = url
            .as_str()
            .into_client_request()
            .map_err(|e| RelayError::communication(format!("failed to build request: {e}")))?;

        // Add auth token — AAD tokens use standard `Authorization` header,
        // SAS tokens use the `ServiceBusAuthorization` header.
        if let Some(provider) = &inner.token_provider {
            let audience = normalize_audience(&inner.address);
            let token = provider.get_token_boxed(&audience, Duration::from_secs(3600)).await?;

            #[cfg(feature = "azure-identity")]
            let header_name = if inner.is_aad {
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

        tracing::info!("Connecting control channel to {}", url);

        let result = tokio_tungstenite::connect_async(request).await;

        match result {
            Ok((ws, _response)) => Ok(ws),
            Err(tokio_tungstenite::tungstenite::Error::Http(response)) => {
                let status = response.status().as_u16();
                let body_text = response
                    .body()
                    .as_ref()
                    .map(|b| String::from_utf8_lossy(b).to_string())
                    .unwrap_or_default();
                match status {
                    401 | 403 => Err(RelayError::AuthorizationFailed(format!(
                        "HTTP {status}: {body_text}"
                    ))),
                    404 => Err(RelayError::EndpointNotFound(format!(
                        "HTTP 404: {body_text}"
                    ))),
                    _ => Err(RelayError::communication(format!(
                        "HTTP {status}: {body_text}"
                    ))),
                }
            }
            Err(e) => Err(e.into()),
        }
    }

    // ------------------------------------------------------------------
    // Control channel message loop
    // ------------------------------------------------------------------

    async fn run_control_channel(
        inner: &Arc<ListenerInner>,
        ws: WebSocketStream<MaybeTlsStream<TcpStream>>,
    ) -> Result<()> {
        let (mut ws_sink, mut ws_stream) = ws.split();

        // Channel for sending messages to the WebSocket sink (used by token
        // renewal and for pong replies).
        let (ws_tx, mut ws_rx) = mpsc::channel::<Message>(16);

        // Spawn a task that forwards messages from ws_tx → ws_sink.
        let sink_handle = tokio::spawn(async move {
            while let Some(msg) = ws_rx.recv().await {
                if ws_sink.send(msg).await.is_err() {
                    break;
                }
            }
            let _ = ws_sink.close().await;
        });

        // Spawn token renewal loop.
        let renewal_inner = inner.clone();
        let renewal_tx = ws_tx.clone();
        let renewal_handle = tokio::spawn(async move {
            Self::token_renewal_loop(renewal_inner, renewal_tx).await;
        });

        let result = Self::read_control_messages(inner, &mut ws_stream, &ws_tx).await;

        // Clean up background tasks.
        renewal_handle.abort();
        sink_handle.abort();

        result
    }

    async fn read_control_messages(
        inner: &Arc<ListenerInner>,
        ws_stream: &mut futures_util::stream::SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>,
        ws_tx: &mpsc::Sender<Message>,
    ) -> Result<()> {
        let mut ping_interval = tokio::time::interval(inner.keep_alive_interval);
        ping_interval.tick().await; // Skip the first immediate tick

        loop {
            tokio::select! {
                msg = ws_stream.next() => {
                    match msg {
                        Some(Ok(Message::Text(text))) => {
                            match serde_json::from_str::<ListenerCommand>(&text) {
                                Ok(cmd) => match cmd {
                                    ListenerCommand::Accept(accept) => {
                                        let inner = inner.clone();
                                        tokio::spawn(async move {
                                            if let Err(e) = Self::handle_accept(&inner, accept).await {
                                                tracing::warn!("Accept handler error: {}", e);
                                            }
                                        });
                                    }
                                    ListenerCommand::Request(request_cmd) => {
                                        let inner = inner.clone();
                                        let ws_tx = ws_tx.clone();
                                        tokio::spawn(async move {
                                            Self::handle_request(&inner, request_cmd, &ws_tx).await;
                                        });
                                    }
                                },
                                Err(e) => {
                                    tracing::warn!("Failed to parse control message: {}", e);
                                }
                            }
                        }
                        Some(Ok(Message::Ping(data))) => {
                            let _ = ws_tx.send(Message::Pong(data)).await;
                        }
                        Some(Ok(Message::Close(_))) | None => {
                            return Ok(());
                        }
                        Some(Err(e)) => {
                            return Err(e.into());
                        }
                        _ => {} // Ignore Pong, Binary, Frame
                    }
                }
                _ = ping_interval.tick() => {
                    if ws_tx.send(Message::Ping(vec![].into())).await.is_err() {
                        tracing::warn!("Control channel ping failed: sink closed");
                        return Err(RelayError::communication("ping send failed"));
                    }
                    tracing::trace!("Control channel ping sent");
                }
                _ = inner.shutdown.notified() => {
                    let _ = ws_tx.send(Message::Close(None)).await;
                    return Ok(());
                }
            }
        }
    }

    // ------------------------------------------------------------------
    // Request handler
    // ------------------------------------------------------------------

    async fn handle_request(
        inner: &Arc<ListenerInner>,
        request_cmd: crate::protocol::RequestCommand,
        ws_tx: &mpsc::Sender<Message>,
    ) {
        use crate::http::{RelayedHttpListenerContext, RelayedHttpListenerRequest};

        let handler = inner.request_handler.read().await;
        let Some(handler) = handler.as_ref() else {
            tracing::debug!("Received HTTP request but no handler is set, ignoring");
            return;
        };

        let request = RelayedHttpListenerRequest::from_command(&request_cmd, None);
        let context = RelayedHttpListenerContext::new(
            request,
            request_cmd.id.clone(),
            Some(request_cmd.address.clone()),
        );

        let response = handler.handle_request_boxed(context).await;
        let response_cmd = response.into_command(request_cmd.id);
        let listener_response = ListenerResponse::Response(response_cmd);

        if let Ok(json) = serde_json::to_string(&listener_response) {
            if ws_tx.send(Message::Text(json.into())).await.is_err() {
                tracing::warn!("Failed to send HTTP response on control channel");
            }
        }
    }

    // ------------------------------------------------------------------
    // Accept handler
    // ------------------------------------------------------------------

    async fn handle_accept(inner: &Arc<ListenerInner>, accept: AcceptCommand) -> Result<()> {
        // 2ms delay before rendezvous connect (ARP race condition workaround, matching C# SDK).
        tokio::time::sleep(Duration::from_millis(2)).await;

        // 20s timeout for the rendezvous WebSocket connection (matching C# SDK).
        use tokio_tungstenite::tungstenite::client::IntoClientRequest;

        let mut request = accept
            .address
            .as_str()
            .into_client_request()
            .map_err(|e| RelayError::communication(format!("invalid rendezvous URL: {e}")))?;

        // Echo Sec-WebSocket-Protocol if present in connect headers
        if let Some(protocol) = accept.connect_headers.get("Sec-WebSocket-Protocol")
            && let Ok(value) = protocol.parse()
        {
            request
                .headers_mut()
                .insert("Sec-WebSocket-Protocol", value);
        }

        let connect = tokio_tungstenite::connect_async(request);
        let (ws, _response) = tokio::time::timeout(Duration::from_secs(20), connect)
            .await
            .map_err(|_| RelayError::Timeout(Duration::from_secs(20)))?
            .map_err(RelayError::from)?;

        let tracking_id = accept.id;
        let stream = HybridConnectionStream::new(ws, tracking_id);

        if inner.accept_tx.send(stream).await.is_err() {
            tracing::debug!("Accept channel closed, dropping connection");
        }

        Ok(())
    }

    // ------------------------------------------------------------------
    // Token renewal
    // ------------------------------------------------------------------

    async fn token_renewal_loop(inner: Arc<ListenerInner>, ws_tx: mpsc::Sender<Message>) {
        let Some(provider) = &inner.token_provider else {
            return;
        };

        loop {
            let audience = protocol::normalize_audience(&inner.address);
            match provider.get_token_boxed(&audience, Duration::from_secs(3600)).await {
                Ok(token) => {
                    let cmd = ListenerResponse::RenewToken(RenewTokenCommand {
                        token: token.token.clone(),
                    });
                    if let Ok(json) = serde_json::to_string(&cmd) {
                        let _ = ws_tx.send(Message::Text(json.into())).await;
                    }

                    // Sleep until near expiry.
                    let until_expiry = token
                        .expires_at
                        .duration_since(std::time::SystemTime::now())
                        .unwrap_or(Duration::from_secs(3600));
                    let sleep_dur = until_expiry.saturating_sub(TOKEN_RENEWAL_MARGIN);
                    if sleep_dur.is_zero() {
                        tokio::time::sleep(Duration::from_secs(30)).await;
                    } else {
                        tokio::time::sleep(sleep_dur).await;
                    }
                }
                Err(e) => {
                    tracing::warn!("Token renewal failed: {}", e);
                    tokio::time::sleep(Duration::from_secs(30)).await;
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

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
    fn normalize_audience_no_hc_prefix() {
        use crate::protocol::normalize_audience;
        let url = Url::parse("wss://mynamespace.servicebus.windows.net/plain").unwrap();
        assert_eq!(
            normalize_audience(&url),
            "http://mynamespace.servicebus.windows.net/plain/"
        );
    }

    #[test]
    fn from_connection_string_missing_endpoint() {
        let cs = "EntityPath=hc1;SharedAccessKeyName=key;SharedAccessKey=secret";
        let err = HybridConnectionListener::from_connection_string(cs).unwrap_err();
        assert!(matches!(err, RelayError::InvalidConnectionString(_)));
    }

    #[test]
    fn from_connection_string_missing_entity_path() {
        let cs = "Endpoint=sb://ns.servicebus.windows.net;SharedAccessKeyName=key;SharedAccessKey=secret";
        let err = HybridConnectionListener::from_connection_string(cs).unwrap_err();
        assert!(matches!(err, RelayError::InvalidConnectionString(_)));
    }

    #[test]
    fn from_connection_string_valid() {
        let cs = "Endpoint=sb://ns.servicebus.windows.net;EntityPath=hc1;SharedAccessKeyName=listen;SharedAccessKey=dGVzdA==";
        let listener = HybridConnectionListener::from_connection_string(cs).unwrap();
        assert_eq!(
            listener.inner.address.as_str(),
            "wss://ns.servicebus.windows.net/$hc/hc1"
        );
        assert!(listener.inner.token_provider.is_some());
    }

    #[test]
    fn from_uri_no_auth() {
        let url = Url::parse("wss://ns.servicebus.windows.net/$hc/hc1").unwrap();
        let listener = HybridConnectionListener::from_uri_no_auth(url.clone());
        assert_eq!(listener.inner.address, url);
        assert!(listener.inner.token_provider.is_none());
    }

    #[test]
    fn status_defaults_to_offline() {
        let url = Url::parse("wss://ns.servicebus.windows.net/$hc/hc1").unwrap();
        let listener = HybridConnectionListener::from_uri_no_auth(url);
        assert_eq!(listener.status(), ConnectionStatus::Offline);
        assert!(!listener.is_online());
    }

    #[test]
    fn reconnect_delays_sequence() {
        assert_eq!(RECONNECT_DELAYS.len(), 6);
        assert_eq!(RECONNECT_DELAYS[0], Duration::from_secs(0));
        assert_eq!(RECONNECT_DELAYS[5], Duration::from_secs(30));
    }

    #[test]
    fn from_uri_with_token_provider() {
        use crate::token_provider::SharedAccessSignatureTokenProvider;
        let url = Url::parse("wss://contoso.servicebus.windows.net/$hc/myconn").unwrap();
        let provider =
            Arc::new(SharedAccessSignatureTokenProvider::new("keyName", "keyValue").unwrap());
        let listener = HybridConnectionListener::from_uri(url.clone(), provider);
        assert_eq!(listener.status(), ConnectionStatus::Offline);
        assert!(!listener.is_online());
    }
}
