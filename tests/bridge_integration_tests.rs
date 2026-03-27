//! End-to-end bridge integration tests.
//!
//! These tests wire `TcpLocalForwardBridge` + `RemoteForwardBridge` directly
//! against `MockRelayServer` and a local TCP echo server, verifying the full
//! azbridge pipeline: local TCP → relay → preamble → remote forwarder → target.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use azure_relay::test_utils::MockRelayServer;
use azure_relay::{HybridConnectionClient, HybridConnectionListener};
use azbridge_lib::preamble::ConnectionMode;
use azbridge_lib::remote_bridge::{RemoteForwardBridge, RemoteForwarder};
use azbridge_lib::tcp::{TcpLocalForwardBridge, TcpRemoteForwarder};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Notify;

/// Start a TCP echo server on an ephemeral port. Returns the bound address.
async fn start_echo_server() -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        loop {
            let (mut stream, _) = match listener.accept().await {
                Ok(s) => s,
                Err(_) => break,
            };
            tokio::spawn(async move {
                let mut buf = vec![0u8; 8192];
                loop {
                    let n = match stream.read(&mut buf).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => n,
                    };
                    if stream.write_all(&buf[..n]).await.is_err() {
                        break;
                    }
                }
            });
        }
    });
    (addr, handle)
}

/// Helper: build a ws:// URL for the mock relay.
fn mock_relay_url(port: u16, entity_path: &str) -> url::Url {
    url::Url::parse(&format!("ws://127.0.0.1:{}/$hc/{}", port, entity_path)).unwrap()
}

/// Full TCP bridge pipeline: local TCP → TcpLocalForwardBridge → mock relay →
/// RemoteForwardBridge → TcpRemoteForwarder → echo server → back.
#[tokio::test]
async fn tcp_bridge_echo_through_mock_relay() {
    // 1. Start infrastructure
    let relay = MockRelayServer::start().await;
    let (echo_addr, echo_handle) = start_echo_server().await;
    let relay_url = mock_relay_url(relay.port, "tcp-bridge");
    let shutdown = Arc::new(Notify::new());

    // 2. Remote side: listener + TcpRemoteForwarder → echo server
    let listener = HybridConnectionListener::from_uri_no_auth(relay_url.clone());
    let remote_fwd = TcpRemoteForwarder::new(echo_addr, "8080".into(), Duration::from_secs(10), None);
    let mut forwarders = HashMap::new();
    forwarders.insert("8080".to_string(), RemoteForwarder::Tcp(remote_fwd));
    let bridge = RemoteForwardBridge::new(listener, forwarders, vec![], "tcp-bridge".into());

    let shutdown2 = shutdown.clone();
    let remote_handle = tokio::spawn(async move {
        bridge.run(shutdown2).await.unwrap();
    });

    // Give the remote bridge time to open the listener
    tokio::time::sleep(Duration::from_millis(300)).await;

    // 3. Local side: TcpLocalForwardBridge → client → relay
    let local_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let local_port = local_listener.local_addr().unwrap();
    drop(local_listener); // Release the port so the bridge can bind it

    let client = Arc::new(HybridConnectionClient::from_uri_no_auth(relay_url));
    let local_bridge = TcpLocalForwardBridge::new(
        local_port,
        "tcp-bridge".into(),
        "8080".into(),
        false,
        Duration::from_secs(10),
    );

    let shutdown3 = shutdown.clone();
    let local_handle = tokio::spawn(async move {
        local_bridge.run(client, shutdown3).await.unwrap();
    });

    // Give the local bridge time to bind
    tokio::time::sleep(Duration::from_millis(200)).await;

    // 4. Connect through the local bridge and verify echo
    let mut tcp_stream = tokio::net::TcpStream::connect(local_port).await.unwrap();
    tcp_stream.write_all(b"hello bridge").await.unwrap();
    tcp_stream.flush().await.unwrap();

    let mut buf = vec![0u8; 1024];
    let n = tokio::time::timeout(Duration::from_secs(5), tcp_stream.read(&mut buf))
        .await
        .expect("read timed out")
        .unwrap();
    assert_eq!(&buf[..n], b"hello bridge");

    // 5. Clean up
    drop(tcp_stream);
    shutdown.notify_waiters();
    let _ = tokio::time::timeout(Duration::from_secs(2), local_handle).await;
    let _ = tokio::time::timeout(Duration::from_secs(2), remote_handle).await;
    echo_handle.abort();
    relay.stop().await;
}

/// Multiple sequential TCP connections through the bridge.
#[tokio::test]
async fn tcp_bridge_multiple_connections() {
    let relay = MockRelayServer::start().await;
    let (echo_addr, echo_handle) = start_echo_server().await;
    let relay_url = mock_relay_url(relay.port, "tcp-multi");
    let shutdown = Arc::new(Notify::new());

    // Remote side
    let listener = HybridConnectionListener::from_uri_no_auth(relay_url.clone());
    let remote_fwd = TcpRemoteForwarder::new(echo_addr, "web".into(), Duration::from_secs(10), None);
    let mut forwarders = HashMap::new();
    forwarders.insert("web".to_string(), RemoteForwarder::Tcp(remote_fwd));
    let bridge = RemoteForwardBridge::new(listener, forwarders, vec![], "tcp-multi".into());

    let shutdown2 = shutdown.clone();
    let remote_handle = tokio::spawn(async move {
        bridge.run(shutdown2).await.unwrap();
    });
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Local side
    let local_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let local_port = local_listener.local_addr().unwrap();
    drop(local_listener);

    let client = Arc::new(HybridConnectionClient::from_uri_no_auth(relay_url));
    let local_bridge = TcpLocalForwardBridge::new(
        local_port, "tcp-multi".into(), "web".into(), false, Duration::from_secs(10),
    );
    let shutdown3 = shutdown.clone();
    let local_handle = tokio::spawn(async move {
        local_bridge.run(client, shutdown3).await.unwrap();
    });
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Send 3 sequential connections
    for i in 0..3 {
        let mut stream = tokio::net::TcpStream::connect(local_port).await.unwrap();
        let msg = format!("msg-{}", i);
        stream.write_all(msg.as_bytes()).await.unwrap();
        stream.flush().await.unwrap();

        let mut buf = vec![0u8; 1024];
        let n = tokio::time::timeout(Duration::from_secs(5), stream.read(&mut buf))
            .await
            .expect("read timed out")
            .unwrap();
        assert_eq!(&buf[..n], msg.as_bytes(), "echo mismatch on connection {}", i);
        drop(stream);
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    shutdown.notify_waiters();
    let _ = tokio::time::timeout(Duration::from_secs(2), local_handle).await;
    let _ = tokio::time::timeout(Duration::from_secs(2), remote_handle).await;
    echo_handle.abort();
    relay.stop().await;
}

/// Preamble mode validation: remote bridge rejects datagram mode on TCP forwarder.
#[tokio::test]
async fn preamble_mode_mismatch_rejected() {
    let relay = MockRelayServer::start().await;
    let (echo_addr, echo_handle) = start_echo_server().await;
    let relay_url = mock_relay_url(relay.port, "mismatch");
    let shutdown = Arc::new(Notify::new());

    let listener = HybridConnectionListener::from_uri_no_auth(relay_url.clone());
    let remote_fwd = TcpRemoteForwarder::new(echo_addr, "tcp".into(), Duration::from_secs(10), None);
    let mut forwarders = HashMap::new();
    forwarders.insert("tcp".to_string(), RemoteForwarder::Tcp(remote_fwd));
    let bridge = RemoteForwardBridge::new(listener, forwarders, vec![], "mismatch".into());

    let shutdown2 = shutdown.clone();
    let remote_handle = tokio::spawn(async move {
        bridge.run(shutdown2).await.unwrap();
    });
    tokio::time::sleep(Duration::from_millis(300)).await;

    // Send a DATAGRAM mode preamble — should be rejected by TCP forwarder
    let client = HybridConnectionClient::from_uri_no_auth(relay_url);
    let mut stream = client.create_connection().await.unwrap();

    let request = azbridge_lib::preamble::PreambleRequest {
        mode: ConnectionMode::Datagram,
        port_name: "tcp".to_string(),
    };
    azbridge_lib::preamble::write_request(&mut stream, &request).await.unwrap();

    let response = azbridge_lib::preamble::read_response(&mut stream).await;
    assert!(response.is_err(), "datagram mode on TCP forwarder should be rejected");

    shutdown.notify_waiters();
    let _ = tokio::time::timeout(Duration::from_secs(2), remote_handle).await;
    echo_handle.abort();
    relay.stop().await;
}

/// Preamble with unknown port name is rejected.
#[tokio::test]
async fn preamble_unknown_port_name_rejected() {
    let relay = MockRelayServer::start().await;
    let (echo_addr, echo_handle) = start_echo_server().await;
    let relay_url = mock_relay_url(relay.port, "unknown-port");
    let shutdown = Arc::new(Notify::new());

    let listener = HybridConnectionListener::from_uri_no_auth(relay_url.clone());
    let remote_fwd = TcpRemoteForwarder::new(echo_addr, "known".into(), Duration::from_secs(10), None);
    let remote_fwd2 = TcpRemoteForwarder::new(echo_addr, "known2".into(), Duration::from_secs(10), None);
    let mut forwarders = HashMap::new();
    forwarders.insert("known".to_string(), RemoteForwarder::Tcp(remote_fwd));
    forwarders.insert("known2".to_string(), RemoteForwarder::Tcp(remote_fwd2));
    let bridge = RemoteForwardBridge::new(listener, forwarders, vec![], "unknown-port".into());

    let shutdown2 = shutdown.clone();
    let remote_handle = tokio::spawn(async move {
        bridge.run(shutdown2).await.unwrap();
    });
    tokio::time::sleep(Duration::from_millis(300)).await;

    let client = HybridConnectionClient::from_uri_no_auth(relay_url);
    let mut stream = client.create_connection().await.unwrap();

    let request = azbridge_lib::preamble::PreambleRequest {
        mode: ConnectionMode::Stream,
        port_name: "nonexistent".to_string(),
    };
    azbridge_lib::preamble::write_request(&mut stream, &request).await.unwrap();

    let response = azbridge_lib::preamble::read_response(&mut stream).await;
    assert!(response.is_err(), "unknown port name should be rejected");

    shutdown.notify_waiters();
    let _ = tokio::time::timeout(Duration::from_secs(2), remote_handle).await;
    echo_handle.abort();
    relay.stop().await;
}
