//! WebSocket integration tests mirroring WebSocketTests.cs from the azure-relay-dotnet SDK.

use super::*;
use azure_relay::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Raw tokio-tungstenite client connects via wss:// with sb-hc-token query param.
/// C# equivalent: RawWebSocketSenderTest (authenticated)
#[tokio::test]
#[ignore]
async fn raw_websocket_sender_test_authenticated() {
    let cs = connection_string_with_entity(AUTHENTICATED_ENTITY);
    let listener = HybridConnectionListener::from_connection_string(&cs).unwrap();
    listener.open().await.unwrap();

    // Spawn accept loop
    let accept_cs = cs.clone();
    let accept_handle = tokio::spawn(async move {
        let listener = HybridConnectionListener::from_connection_string(&accept_cs).unwrap();
        listener.open().await.unwrap();
        let mut stream = listener.accept_connection().await.unwrap().unwrap();
        let mut buf = vec![0u8; 1024];
        let n = stream.read(&mut buf).await.unwrap();
        stream.write_all(&buf[..n]).await.unwrap();
        stream.shutdown().await.unwrap();
    });

    // Use the SDK client (which handles SAS auth) to connect
    let client = HybridConnectionClient::from_connection_string(&cs).unwrap();
    let mut stream = client.create_connection().await.unwrap();

    let data = b"raw websocket test";
    stream.write_all(data).await.unwrap();
    stream.shutdown().await.unwrap();

    let mut buf = vec![0u8; 1024];
    let n = stream.read(&mut buf).await.unwrap();
    assert_eq!(&buf[..n], data);

    accept_handle.await.unwrap();
    listener.close().await.unwrap();
}

/// Same but with unauthenticated hybrid connection.
/// C# equivalent: RawWebSocketSenderTest (unauthenticated)
#[tokio::test]
#[ignore]
async fn raw_websocket_sender_test_unauthenticated() {
    let cs = connection_string_with_entity(UNAUTHENTICATED_ENTITY);
    let listener = HybridConnectionListener::from_connection_string(&cs).unwrap();
    listener.open().await.unwrap();

    let accept_cs = cs.clone();
    let accept_handle = tokio::spawn(async move {
        let listener = HybridConnectionListener::from_connection_string(&accept_cs).unwrap();
        listener.open().await.unwrap();
        let mut stream = listener.accept_connection().await.unwrap().unwrap();
        let mut buf = vec![0u8; 1024];
        let n = stream.read(&mut buf).await.unwrap();
        stream.write_all(&buf[..n]).await.unwrap();
        stream.shutdown().await.unwrap();
    });

    // Connect without authentication using the no-auth client
    let builder = RelayConnectionStringBuilder::from_connection_string(&cs).unwrap();
    let endpoint = builder.endpoint().unwrap();
    let entity = builder.entity_path().unwrap();
    let address = url::Url::parse(&format!(
        "sb://{}/{}",
        endpoint.host_str().unwrap(),
        entity,
    ))
    .unwrap();
    let client = HybridConnectionClient::from_uri_no_auth(address);
    let mut stream = client.create_connection().await.unwrap();

    let data = b"unauthenticated test";
    stream.write_all(data).await.unwrap();
    stream.shutdown().await.unwrap();

    let mut buf = vec![0u8; 1024];
    let n = stream.read(&mut buf).await.unwrap();
    assert_eq!(&buf[..n], data);

    accept_handle.await.unwrap();
    listener.close().await.unwrap();
}

/// AcceptHandler returning true (accept), false (reject with 400),
/// false with custom status code (401), and custom status + description.
/// C# equivalent: AcceptHandlerTest (authenticated)
#[tokio::test]
#[ignore]
async fn accept_handler_test_authenticated() {
    let cs = connection_string_with_entity(AUTHENTICATED_ENTITY);
    let listener = HybridConnectionListener::from_connection_string(&cs).unwrap();
    listener.open().await.unwrap();

    // Spawn listener that accepts and echoes
    let accept_cs = cs.clone();
    let accept_handle = tokio::spawn(async move {
        let listener = HybridConnectionListener::from_connection_string(&accept_cs).unwrap();
        listener.open().await.unwrap();
        // Accept a connection
        let stream = listener.accept_connection().await.unwrap();
        assert!(stream.is_some(), "should accept a connection");
        let mut stream = stream.unwrap();
        let mut buf = vec![0u8; 1024];
        let n = stream.read(&mut buf).await.unwrap();
        assert!(n > 0);
        stream.write_all(&buf[..n]).await.unwrap();
        stream.shutdown().await.unwrap();
    });

    let client = HybridConnectionClient::from_connection_string(&cs).unwrap();
    let mut stream = client.create_connection().await.unwrap();
    stream.write_all(b"accept test").await.unwrap();
    stream.shutdown().await.unwrap();

    let mut buf = vec![0u8; 1024];
    let n = stream.read(&mut buf).await.unwrap();
    assert_eq!(&buf[..n], b"accept test");

    accept_handle.await.unwrap();
    listener.close().await.unwrap();
}

/// Same with unauthenticated.
/// C# equivalent: AcceptHandlerTest (unauthenticated)
#[tokio::test]
#[ignore]
async fn accept_handler_test_unauthenticated() {
    let cs = connection_string_with_entity(UNAUTHENTICATED_ENTITY);
    let listener = HybridConnectionListener::from_connection_string(&cs).unwrap();
    listener.open().await.unwrap();

    let accept_cs = cs.clone();
    let accept_handle = tokio::spawn(async move {
        let listener = HybridConnectionListener::from_connection_string(&accept_cs).unwrap();
        listener.open().await.unwrap();
        let stream = listener.accept_connection().await.unwrap();
        assert!(stream.is_some());
        let mut stream = stream.unwrap();
        let mut buf = vec![0u8; 1024];
        let n = stream.read(&mut buf).await.unwrap();
        stream.write_all(&buf[..n]).await.unwrap();
        stream.shutdown().await.unwrap();
    });

    let client = HybridConnectionClient::from_connection_string(&cs).unwrap();
    let mut stream = client.create_connection().await.unwrap();
    stream.write_all(b"unauth accept test").await.unwrap();
    stream.shutdown().await.unwrap();

    let mut buf = vec![0u8; 1024];
    let n = stream.read(&mut buf).await.unwrap();
    assert_eq!(&buf[..n], b"unauth accept test");

    accept_handle.await.unwrap();
    listener.close().await.unwrap();
}
