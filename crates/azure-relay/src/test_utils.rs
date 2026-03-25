//! Mock Azure Relay server for integration testing.
//!
//! Provides [`MockRelayServer`] — an in-process WebSocket server that simulates
//! the Azure Relay Hybrid Connections service, enabling integration tests
//! without a live Azure namespace.

use std::collections::HashMap;
use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio::sync::{oneshot, Mutex, Notify};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;

/// Concrete server-side WebSocket stream type (no TLS).
type ServerWsStream = tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>;

/// Shared state for the mock relay.
struct RelayState {
    /// Entity path → control-channel message sender for each registered listener.
    listeners: HashMap<String, tokio::sync::mpsc::UnboundedSender<Message>>,
    /// Rendezvous ID → oneshot sender to deliver the listener's accept connection.
    pending_rendezvous: HashMap<String, oneshot::Sender<ServerWsStream>>,
}

/// A mock Azure Relay server for integration testing.
///
/// Listens on `127.0.0.1` with an OS-assigned port and handles three kinds of
/// WebSocket connections (distinguished by the `sb-hc-action` query parameter):
///
/// - **`listen`** — control channel. The mock keeps it alive and sends `accept`
///   commands when senders arrive.
/// - **`connect`** — sender connection. The mock generates a rendezvous URL,
///   notifies the listener, and waits for the listener to accept.
/// - **`accept`** — rendezvous connection from the listener. The mock bridges
///   the sender and listener WebSocket streams.
pub struct MockRelayServer {
    /// The port the mock server is listening on.
    pub port: u16,
    shutdown: Arc<Notify>,
    handle: JoinHandle<()>,
}

impl MockRelayServer {
    /// Start the mock relay server on an OS-assigned port.
    pub async fn start() -> Self {
        let tcp = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = tcp.local_addr().unwrap().port();
        let shutdown = Arc::new(Notify::new());
        let shutdown2 = shutdown.clone();

        let state = Arc::new(Mutex::new(RelayState {
            listeners: HashMap::new(),
            pending_rendezvous: HashMap::new(),
        }));

        let handle = tokio::spawn(async move {
            Self::accept_loop(tcp, state, shutdown2, port).await;
        });

        Self {
            port,
            shutdown,
            handle,
        }
    }

    /// Build a connection string pointing at this mock server.
    pub fn connection_string(&self, entity_path: &str) -> String {
        format!(
            "Endpoint=sb://127.0.0.1:{};EntityPath={};SharedAccessKeyName=mock;SharedAccessKey=bW9jaw==",
            self.port, entity_path
        )
    }

    /// Shut down the mock server gracefully.
    pub async fn stop(self) {
        self.shutdown.notify_one();
        let _ = self.handle.await;
    }

    // ----- accept loop -----------------------------------------------------

    async fn accept_loop(
        tcp: TcpListener,
        state: Arc<Mutex<RelayState>>,
        shutdown: Arc<Notify>,
        port: u16,
    ) {
        loop {
            tokio::select! {
                result = tcp.accept() => {
                    if let Ok((stream, _)) = result {
                        let st = state.clone();
                        tokio::spawn(async move {
                            if let Err(e) = Self::handle_connection(stream, st, port).await {
                                tracing::warn!("mock relay error: {e}");
                            }
                        });
                    }
                }
                _ = shutdown.notified() => break,
            }
        }
    }

    // ----- connection dispatch ----------------------------------------------

    async fn handle_connection(
        stream: tokio::net::TcpStream,
        state: Arc<Mutex<RelayState>>,
        port: u16,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // Capture the request URI during the WebSocket handshake.
        let uri_holder = Arc::new(std::sync::Mutex::new(String::new()));
        let uri_clone = uri_holder.clone();

        let ws = tokio_tungstenite::accept_hdr_async(stream, move |req: &http::Request<()>, resp| {
            *uri_clone.lock().unwrap() = req.uri().to_string();
            Ok(resp)
        })
        .await?;

        let uri_str = uri_holder.lock().unwrap().clone();
        let url = url::Url::parse(&format!("ws://localhost{}", uri_str))?;

        let params: HashMap<String, String> = url.query_pairs().into_owned().collect();
        let action = params.get("sb-hc-action").cloned().unwrap_or_default();
        let entity_path = url
            .path()
            .strip_prefix("/$hc/")
            .unwrap_or(url.path())
            .to_string();

        match action.as_str() {
            "listen" => Self::on_listen(ws, entity_path, state).await,
            "connect" => Self::on_connect(ws, entity_path, state, port).await,
            "accept" => Self::on_accept(ws, params, state).await,
            other => Err(format!("unknown action: {other}").into()),
        }
    }

    // ----- listener control channel -----------------------------------------

    async fn on_listen(
        ws: ServerWsStream,
        entity_path: String,
        state: Arc<Mutex<RelayState>>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let (sink, mut stream) = ws.split();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Message>();
        let pong_tx = tx.clone();

        // Forward control messages → listener WebSocket sink.
        let fwd = tokio::spawn(async move {
            let mut sink = sink;
            while let Some(msg) = rx.recv().await {
                if sink.send(msg).await.is_err() {
                    break;
                }
            }
        });

        // Register this listener's control channel.
        state
            .lock()
            .await
            .listeners
            .insert(entity_path.clone(), tx);

        // Keep the control channel alive; respond to pings.
        while let Some(msg) = stream.next().await {
            match msg {
                Ok(Message::Ping(data)) => {
                    let _ = pong_tx.send(Message::Pong(data));
                }
                Ok(Message::Close(_)) | Err(_) => break,
                _ => {}
            }
        }

        state.lock().await.listeners.remove(&entity_path);
        fwd.abort();
        Ok(())
    }

    // ----- sender connection ------------------------------------------------

    async fn on_connect(
        ws: ServerWsStream,
        entity_path: String,
        state: Arc<Mutex<RelayState>>,
        port: u16,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let rendezvous_id = uuid::Uuid::new_v4().to_string();
        let rendezvous_url = format!(
            "ws://127.0.0.1:{}/$hc/{}?sb-hc-action=accept&sb-hc-id={}",
            port, entity_path, rendezvous_id
        );

        let (tx, rx) = oneshot::channel::<ServerWsStream>();

        {
            let mut s = state.lock().await;
            s.pending_rendezvous.insert(rendezvous_id.clone(), tx);

            let listener_tx = s
                .listeners
                .get(&entity_path)
                .ok_or("no listener for entity path")?;

            let cmd = serde_json::json!({
                "accept": {
                    "address": rendezvous_url,
                    "id": rendezvous_id,
                    "connectHeaders": {},
                    "remoteEndpoint": { "address": "127.0.0.1", "port": 0 }
                }
            });
            listener_tx.send(Message::Text(cmd.to_string().into()))?;
        }

        // Wait for the listener to complete the rendezvous.
        let listener_ws = tokio::time::timeout(std::time::Duration::from_secs(30), rx)
            .await
            .map_err(|_| "rendezvous timeout")?
            .map_err(|_| "rendezvous cancelled")?;

        Self::bridge(ws, listener_ws).await;
        Ok(())
    }

    // ----- rendezvous accept ------------------------------------------------

    async fn on_accept(
        ws: ServerWsStream,
        params: HashMap<String, String>,
        state: Arc<Mutex<RelayState>>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let id = params.get("sb-hc-id").ok_or("missing sb-hc-id")?;
        let tx = state
            .lock()
            .await
            .pending_rendezvous
            .remove(id)
            .ok_or("no pending rendezvous")?;
        tx.send(ws).map_err(|_| "rendezvous send failed")?;
        Ok(())
    }

    // ----- WebSocket bridge -------------------------------------------------

    /// Forward messages between two WebSocket streams in both directions.
    async fn bridge(a: ServerWsStream, b: ServerWsStream) {
        let (mut a_w, mut a_r) = a.split();
        let (mut b_w, mut b_r) = b.split();

        let a_to_b = tokio::spawn(async move {
            while let Some(Ok(msg)) = a_r.next().await {
                let done = matches!(msg, Message::Close(_));
                if b_w.send(msg).await.is_err() || done {
                    break;
                }
            }
        });

        let b_to_a = tokio::spawn(async move {
            while let Some(Ok(msg)) = b_r.next().await {
                let done = matches!(msg, Message::Close(_));
                if a_w.send(msg).await.is_err() || done {
                    break;
                }
            }
        });

        let _ = tokio::join!(a_to_b, b_to_a);
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn mock_relay_starts_and_stops() {
        let server = MockRelayServer::start().await;
        assert!(server.port > 0);
        server.stop().await;
    }

    #[tokio::test]
    async fn mock_relay_connection_string() {
        let server = MockRelayServer::start().await;
        let cs = server.connection_string("my-entity");
        assert!(cs.contains(&server.port.to_string()));
        assert!(cs.contains("my-entity"));
        assert!(cs.contains("Endpoint=sb://"));
        server.stop().await;
    }

    #[tokio::test]
    async fn mock_relay_echo_through_bridge() {
        let server = MockRelayServer::start().await;
        let port = server.port;

        // 1. Connect a "listener" control channel.
        let listener_url = format!(
            "ws://127.0.0.1:{}/$hc/test-entity?sb-hc-action=listen&sb-hc-id=listener-1",
            port
        );
        let (mut listener_ws, _) =
            tokio_tungstenite::connect_async(listener_url).await.unwrap();

        // 2. Connect a "sender" (spawned — the mock blocks until rendezvous).
        let sender_url = format!(
            "ws://127.0.0.1:{}/$hc/test-entity?sb-hc-action=connect&sb-hc-id=sender-1",
            port
        );
        let sender_task = tokio::spawn(async move {
            tokio_tungstenite::connect_async(sender_url).await.unwrap()
        });

        // 3. Read the accept command from the listener's control channel.
        let accept_msg = listener_ws.next().await.unwrap().unwrap();
        let accept_text = match accept_msg {
            Message::Text(t) => t.to_string(),
            other => panic!("expected text message, got: {other:?}"),
        };
        let accept_cmd: serde_json::Value = serde_json::from_str(&accept_text).unwrap();
        let rendezvous_url = accept_cmd["accept"]["address"].as_str().unwrap();

        // 4. Listener connects to the rendezvous URL.
        let (mut listener_data_ws, _) =
            tokio_tungstenite::connect_async(rendezvous_url).await.unwrap();

        // 5. Sender connection completes now that rendezvous is done.
        let (mut sender_ws, _) = sender_task.await.unwrap();

        // 6. sender → listener
        sender_ws
            .send(Message::Text("hello from sender".to_string().into()))
            .await
            .unwrap();
        let msg = listener_data_ws.next().await.unwrap().unwrap();
        assert_eq!(msg, Message::Text("hello from sender".to_string().into()));

        // 7. listener → sender
        listener_data_ws
            .send(Message::Text("hello from listener".to_string().into()))
            .await
            .unwrap();
        let msg = sender_ws.next().await.unwrap().unwrap();
        assert_eq!(
            msg,
            Message::Text("hello from listener".to_string().into())
        );

        // 8. Clean up.
        let _ = sender_ws.send(Message::Close(None)).await;
        let _ = listener_data_ws.send(Message::Close(None)).await;
        drop(listener_ws);

        server.stop().await;
    }
}
