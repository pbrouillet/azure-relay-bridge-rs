//! Mock-server protocol tests that verify full SDK protocol flow without a live Azure Relay.
//!
//! These tests use `MockRelayServer` to simulate the Azure Relay service and
//! exercise the real `HybridConnectionClient` and `HybridConnectionListener`
//! through the complete protocol flow:
//!
//! - Control channel (listen action)
//! - Sender connection (connect action)
//! - Accept command dispatch
//! - Rendezvous WebSocket handshake
//! - Bidirectional data transfer via `HybridConnectionStream`

use azure_relay::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Helper: build a ws:// URL for the mock server (no TLS).
fn mock_relay_url(port: u16, entity_path: &str) -> url::Url {
    url::Url::parse(&format!(
        "ws://127.0.0.1:{}/$hc/{}",
        port, entity_path
    ))
    .unwrap()
}

/// Full protocol flow: listener opens control channel, client connects,
/// accept command dispatched, rendezvous completes, data flows end-to-end.
#[tokio::test]
async fn client_listener_echo_through_mock() {
    let server = azure_relay::test_utils::MockRelayServer::start().await;
    let url = mock_relay_url(server.port, "echo-test");

    let listener = HybridConnectionListener::from_uri_no_auth(url.clone());
    listener.open().await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // Spawn listener accept — reads data, echoes it, then closes.
    let listener_clone = listener.clone();
    let accept_handle = tokio::spawn(async move {
        let mut stream = listener_clone.accept_connection().await.unwrap().unwrap();
        let mut buf = vec![0u8; 1024];
        let n = stream.read(&mut buf).await.unwrap();
        assert!(n > 0, "expected data from client");
        stream.write_all(&buf[..n]).await.unwrap();
        stream.flush().await.unwrap();
        stream.close().await.unwrap();
        n
    });

    // Client connects, sends data, reads echo
    let client = HybridConnectionClient::from_uri_no_auth(url);
    let mut stream = client.create_connection().await.unwrap();

    let data = b"Hello through mock relay!";
    stream.write_all(data).await.unwrap();
    stream.flush().await.unwrap();

    // Read echo before closing
    let mut buf = vec![0u8; 1024];
    let n = stream.read(&mut buf).await.unwrap();
    assert_eq!(&buf[..n], data);

    stream.close().await.unwrap();

    let echoed = accept_handle.await.unwrap();
    assert_eq!(echoed, data.len());

    listener.close().await.unwrap();
    server.stop().await;
}

/// Bidirectional data: client and listener both send and receive.
#[tokio::test]
async fn bidirectional_data_through_mock() {
    let server = azure_relay::test_utils::MockRelayServer::start().await;
    let url = mock_relay_url(server.port, "bidir-test");

    let listener = HybridConnectionListener::from_uri_no_auth(url.clone());
    listener.open().await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    let listener_clone = listener.clone();
    let accept_handle = tokio::spawn(async move {
        let mut stream = listener_clone.accept_connection().await.unwrap().unwrap();
        // Listener sends first
        stream.write_all(b"from-listener").await.unwrap();
        stream.flush().await.unwrap();
        // Listener reads client's response
        let mut buf = vec![0u8; 1024];
        let n = stream.read(&mut buf).await.unwrap();
        assert_eq!(&buf[..n], b"from-client");
        stream.close().await.unwrap();
    });

    let client = HybridConnectionClient::from_uri_no_auth(url);
    let mut stream = client.create_connection().await.unwrap();

    // Client reads listener's message first
    let mut buf = vec![0u8; 1024];
    let n = stream.read(&mut buf).await.unwrap();
    assert_eq!(&buf[..n], b"from-listener");

    // Client sends back
    stream.write_all(b"from-client").await.unwrap();
    stream.flush().await.unwrap();
    stream.close().await.unwrap();

    accept_handle.await.unwrap();
    listener.close().await.unwrap();
    server.stop().await;
}

/// Multiple sequential connections through the same listener.
#[tokio::test]
async fn multiple_sequential_connections() {
    let server = azure_relay::test_utils::MockRelayServer::start().await;
    let url = mock_relay_url(server.port, "multi-seq-test");

    let listener = HybridConnectionListener::from_uri_no_auth(url.clone());
    listener.open().await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    for i in 0..3 {
        let listener_clone = listener.clone();
        let accept_handle = tokio::spawn(async move {
            let mut stream = listener_clone.accept_connection().await.unwrap().unwrap();
            let mut buf = vec![0u8; 1024];
            let n = stream.read(&mut buf).await.unwrap();
            stream.write_all(&buf[..n]).await.unwrap();
            stream.flush().await.unwrap();
            stream.close().await.unwrap();
        });

        let client = HybridConnectionClient::from_uri_no_auth(url.clone());
        let mut stream = client.create_connection().await.unwrap();

        let msg = format!("message-{}", i);
        stream.write_all(msg.as_bytes()).await.unwrap();
        stream.flush().await.unwrap();

        let mut buf = vec![0u8; 1024];
        let n = stream.read(&mut buf).await.unwrap();
        assert_eq!(&buf[..n], msg.as_bytes(), "echo mismatch on iteration {}", i);

        stream.close().await.unwrap();
        accept_handle.await.unwrap();
    }

    listener.close().await.unwrap();
    server.stop().await;
}

/// Concurrent connections: 5 clients all connect simultaneously.
#[tokio::test]
async fn concurrent_connections_through_mock() {
    let server = azure_relay::test_utils::MockRelayServer::start().await;
    let url = mock_relay_url(server.port, "concurrent-test");

    let listener = HybridConnectionListener::from_uri_no_auth(url.clone());
    listener.open().await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    let count = 5u32;

    // Spawn accept loop
    let listener_clone = listener.clone();
    let accept_handle = tokio::spawn(async move {
        let mut handles = Vec::new();
        for _ in 0..count {
            let stream = listener_clone.accept_connection().await.unwrap().unwrap();
            handles.push(tokio::spawn(async move {
                let mut stream = stream;
                let mut buf = vec![0u8; 1024];
                let n = stream.read(&mut buf).await.unwrap();
                if n > 0 {
                    stream.write_all(&buf[..n]).await.unwrap();
                    stream.flush().await.unwrap();
                }
                stream.close().await.unwrap();
            }));
        }
        for h in handles {
            h.await.unwrap();
        }
    });

    // Launch concurrent clients
    let mut client_handles = Vec::new();
    for i in 0..count {
        let url = url.clone();
        client_handles.push(tokio::spawn(async move {
            let client = HybridConnectionClient::from_uri_no_auth(url);
            let mut stream = client.create_connection().await.unwrap();
            let msg = format!("concurrent-{}", i);
            stream.write_all(msg.as_bytes()).await.unwrap();
            stream.flush().await.unwrap();
            let mut buf = vec![0u8; 1024];
            let n = stream.read(&mut buf).await.unwrap();
            assert_eq!(&buf[..n], msg.as_bytes());
            stream.close().await.unwrap();
        }));
    }

    for h in client_handles {
        h.await.unwrap();
    }

    accept_handle.await.unwrap();
    listener.close().await.unwrap();
    server.stop().await;
}

/// Large data transfer (64KB) through the mock relay verifies stream buffering.
#[tokio::test]
async fn large_data_transfer_through_mock() {
    let server = azure_relay::test_utils::MockRelayServer::start().await;
    let url = mock_relay_url(server.port, "large-data-test");

    let listener = HybridConnectionListener::from_uri_no_auth(url.clone());
    listener.open().await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    let data_size = 64 * 1024; // 64KB
    let send_data: Vec<u8> = (0..data_size).map(|i| (i % 256) as u8).collect();
    let expected = send_data.clone();

    let listener_clone = listener.clone();
    let accept_handle = tokio::spawn(async move {
        let mut stream = listener_clone.accept_connection().await.unwrap().unwrap();
        // Read fixed amount of data (we know the size)
        let mut received = vec![0u8; data_size];
        let mut total = 0;
        while total < data_size {
            let n = stream.read(&mut received[total..]).await.unwrap();
            if n == 0 {
                break;
            }
            total += n;
        }
        // Echo it back
        stream.write_all(&received[..total]).await.unwrap();
        stream.flush().await.unwrap();
        stream.close().await.unwrap();
        total
    });

    let client = HybridConnectionClient::from_uri_no_auth(url);
    let mut stream = client.create_connection().await.unwrap();

    // Send all data
    stream.write_all(&send_data).await.unwrap();
    stream.flush().await.unwrap();

    // Read the echo
    let mut received = vec![0u8; data_size];
    let mut total = 0;
    while total < data_size {
        let n = stream.read(&mut received[total..]).await.unwrap();
        if n == 0 {
            break;
        }
        total += n;
    }

    assert_eq!(total, data_size);
    assert_eq!(received, expected);

    stream.close().await.unwrap();

    let listener_received = accept_handle.await.unwrap();
    assert_eq!(listener_received, data_size);

    listener.close().await.unwrap();
    server.stop().await;
}
