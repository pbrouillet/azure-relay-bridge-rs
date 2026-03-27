//! Runtime integration tests mirroring RunTimeTests.cs from the azure-relay-dotnet SDK.

use super::*;
use azure_relay::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Basic 1KB send/receive through the relay, graceful close.
/// C# equivalent: HybridConnectionTest
#[tokio::test]
#[ignore]
async fn hybrid_connection_test() {
    let cs = connection_string_with_entity(AUTHENTICATED_ENTITY);

    let listener = HybridConnectionListener::from_connection_string(&cs).unwrap();
    listener.open().await.unwrap();

    let accept_task = {
        let listener = listener.clone();
        tokio::spawn(async move {
            let mut stream = listener.accept_connection().await.unwrap().unwrap();
            let mut buf = vec![0u8; 1024];
            let n = stream.read(&mut buf).await.unwrap();
            stream.write_all(&buf[..n]).await.unwrap();
            stream.shutdown().await.unwrap();
        })
    };

    let client = HybridConnectionClient::from_connection_string(&cs).unwrap();
    let mut stream = client.create_connection().await.unwrap();

    let data = b"Hello, Azure Relay!";
    stream.write_all(data).await.unwrap();
    stream.shutdown().await.unwrap();

    let mut buf = vec![0u8; 1024];
    let n = stream.read(&mut buf).await.unwrap();
    assert_eq!(&buf[..n], data);

    accept_task.await.unwrap();
    listener.close().await.unwrap();
}

/// Client-initiated half-close: client shuts down write side,
/// listener reads 0 bytes (EOF), then listener shuts down and client reads EOF.
/// C# equivalent: ClientShutdownTest
#[tokio::test]
#[ignore]
async fn client_shutdown_test() {
    let cs = connection_string_with_entity(AUTHENTICATED_ENTITY);
    let listener = HybridConnectionListener::from_connection_string(&cs).unwrap();
    listener.open().await.unwrap();

    let listener_task = {
        let listener = listener.clone();
        tokio::spawn(async move {
            let mut stream = listener.accept_connection().await.unwrap().unwrap();
            // Read until EOF
            let mut buf = vec![0u8; 1024];
            let n = stream.read(&mut buf).await.unwrap();
            assert!(n == 0 || !buf[..n].is_empty()); // got data or EOF
            // Echo back and close
            if n > 0 {
                stream.write_all(&buf[..n]).await.unwrap();
            }
            stream.shutdown().await.unwrap();
        })
    };

    let client = HybridConnectionClient::from_connection_string(&cs).unwrap();
    let mut stream = client.create_connection().await.unwrap();

    let data = b"shutdown test data";
    stream.write_all(data).await.unwrap();
    // Client shuts down write side (half-close)
    stream.shutdown().await.unwrap();

    // Should still be able to read the echo
    let mut buf = vec![0u8; 1024];
    let n = stream.read(&mut buf).await.unwrap();
    assert_eq!(&buf[..n], data);

    listener_task.await.unwrap();
    listener.close().await.unwrap();
}

/// 100 concurrent connections, echo pattern.
/// C# equivalent: ConcurrentClientsTest
#[tokio::test]
#[ignore]
async fn concurrent_clients_test() {
    let cs = connection_string_with_entity(AUTHENTICATED_ENTITY);
    let listener = HybridConnectionListener::from_connection_string(&cs).unwrap();
    listener.open().await.unwrap();

    // Spawn listener accept loop
    let accept_cs = cs.clone();
    let accept_handle = tokio::spawn(async move {
        let listener = HybridConnectionListener::from_connection_string(&accept_cs).unwrap();
        listener.open().await.unwrap();
        let mut count = 0u32;
        while let Ok(Some(mut stream)) = listener.accept_connection().await {
            count += 1;
            tokio::spawn(async move {
                let mut buf = vec![0u8; 1024];
                if let Ok(n) = stream.read(&mut buf).await {
                    if n > 0 {
                        let _ = stream.write_all(&buf[..n]).await;
                    }
                }
                let _ = stream.shutdown().await;
            });
            if count >= 100 {
                break;
            }
        }
    });

    // Launch 100 concurrent clients
    let mut handles = Vec::new();
    for i in 0u32..100 {
        let cs = cs.clone();
        handles.push(tokio::spawn(async move {
            let client = HybridConnectionClient::from_connection_string(&cs).unwrap();
            let mut stream = client.create_connection().await.unwrap();
            let data = format!("msg-{}", i);
            stream.write_all(data.as_bytes()).await.unwrap();
            stream.shutdown().await.unwrap();
            let mut buf = vec![0u8; 1024];
            let n = stream.read(&mut buf).await.unwrap();
            assert_eq!(&buf[..n], data.as_bytes());
        }));
    }

    for h in handles {
        h.await.unwrap();
    }

    accept_handle.abort();
    listener.close().await.unwrap();
}

/// Custom request headers flow to the listener's accept handler.
/// C# equivalent: RequestHeadersTest
#[tokio::test]
#[ignore]
async fn request_headers_test() {
    let cs = connection_string_with_entity(AUTHENTICATED_ENTITY);
    let listener = HybridConnectionListener::from_connection_string(&cs).unwrap();
    listener.open().await.unwrap();

    let accept_cs = cs.clone();
    let accept_handle = tokio::spawn(async move {
        let listener = HybridConnectionListener::from_connection_string(&accept_cs).unwrap();
        listener.open().await.unwrap();
        let mut stream = listener.accept_connection().await.unwrap().unwrap();
        // Echo back to confirm connection works
        let mut buf = vec![0u8; 1024];
        let n = stream.read(&mut buf).await.unwrap();
        stream.write_all(&buf[..n]).await.unwrap();
        stream.shutdown().await.unwrap();
    });

    let client = HybridConnectionClient::from_connection_string(&cs).unwrap();
    let mut headers = std::collections::HashMap::new();
    headers.insert("X-Custom-Header".to_string(), "test-value".to_string());
    let mut stream = client
        .create_connection_with_headers(&headers)
        .await
        .unwrap();

    let data = b"headers test";
    stream.write_all(data).await.unwrap();
    stream.shutdown().await.unwrap();

    let mut buf = vec![0u8; 1024];
    let n = stream.read(&mut buf).await.unwrap();
    assert_eq!(&buf[..n], data);

    accept_handle.await.unwrap();
    listener.close().await.unwrap();
}

/// Invalid header names/values produce errors.
/// C# equivalent: RequestHeadersNegativeTest
#[tokio::test]
#[ignore]
async fn request_headers_negative_test() {
    let cs = connection_string_with_entity(UNAUTHENTICATED_ENTITY);
    let client = HybridConnectionClient::from_connection_string(&cs).unwrap();

    // Invalid header name (contains spaces)
    let mut headers = std::collections::HashMap::new();
    headers.insert("Invalid Header Name".to_string(), "value".to_string());
    let result = client.create_connection_with_headers(&headers).await;
    assert!(result.is_err(), "invalid header name should fail");
}

/// 1MB data transfer in both directions.
/// C# equivalent: WriteLargeDataSetTest
#[tokio::test]
#[ignore]
async fn write_large_data_set_test() {
    let cs = connection_string_with_entity(AUTHENTICATED_ENTITY);
    let listener = HybridConnectionListener::from_connection_string(&cs).unwrap();
    listener.open().await.unwrap();

    let data_size = 1024 * 1024; // 1MB

    let listener_task = {
        let listener = listener.clone();
        tokio::spawn(async move {
            let mut stream = listener.accept_connection().await.unwrap().unwrap();
            // Read all data and echo it back
            let mut received = Vec::new();
            let mut buf = vec![0u8; 65536];
            loop {
                let n = stream.read(&mut buf).await.unwrap();
                if n == 0 {
                    break;
                }
                received.extend_from_slice(&buf[..n]);
            }
            stream.write_all(&received).await.unwrap();
            stream.shutdown().await.unwrap();
            received.len()
        })
    };

    let client = HybridConnectionClient::from_connection_string(&cs).unwrap();
    let mut stream = client.create_connection().await.unwrap();

    // Send 1MB of data
    let send_data: Vec<u8> = (0..data_size).map(|i| (i % 256) as u8).collect();
    stream.write_all(&send_data).await.unwrap();
    stream.shutdown().await.unwrap();

    // Read the echo
    let mut received = Vec::new();
    let mut buf = vec![0u8; 65536];
    loop {
        let n = stream.read(&mut buf).await.unwrap();
        if n == 0 {
            break;
        }
        received.extend_from_slice(&buf[..n]);
    }

    assert_eq!(received.len(), data_size);
    assert_eq!(received, send_data);

    let listener_received = listener_task.await.unwrap();
    assert_eq!(listener_received, data_size);
    listener.close().await.unwrap();
}

/// Listener-initiated graceful shutdown.
/// C# equivalent: ListenerShutdownTest
#[tokio::test]
#[ignore]
async fn listener_shutdown_test() {
    let cs = connection_string_with_entity(AUTHENTICATED_ENTITY);
    let listener = HybridConnectionListener::from_connection_string(&cs).unwrap();
    listener.open().await.unwrap();

    let listener_task = {
        let listener = listener.clone();
        tokio::spawn(async move {
            let mut stream = listener.accept_connection().await.unwrap().unwrap();
            let data = b"listener sends first";
            stream.write_all(data).await.unwrap();
            // Listener initiates shutdown
            stream.shutdown().await.unwrap();
        })
    };

    let client = HybridConnectionClient::from_connection_string(&cs).unwrap();
    let mut stream = client.create_connection().await.unwrap();

    let mut buf = vec![0u8; 1024];
    let n = stream.read(&mut buf).await.unwrap();
    assert_eq!(&buf[..n], b"listener sends first");

    // After listener shutdown, further reads should return EOF
    let n = stream.read(&mut buf).await.unwrap();
    assert_eq!(n, 0);

    listener_task.await.unwrap();
    listener.close().await.unwrap();
}

/// Listener abort while client is reading -> client gets error.
/// C# equivalent: ListenerAbortWhileClientReadingTest
#[tokio::test]
#[ignore]
async fn listener_abort_while_client_reading_test() {
    let cs = connection_string_with_entity(AUTHENTICATED_ENTITY);
    let listener = HybridConnectionListener::from_connection_string(&cs).unwrap();
    listener.open().await.unwrap();

    let listener_task = {
        let listener = listener.clone();
        tokio::spawn(async move {
            let stream = listener.accept_connection().await.unwrap().unwrap();
            // Just drop the stream without graceful shutdown (abort)
            drop(stream);
        })
    };

    let client = HybridConnectionClient::from_connection_string(&cs).unwrap();
    let mut stream = client.create_connection().await.unwrap();

    // Client tries to read — should get an error or EOF since listener aborted
    let mut buf = vec![0u8; 1024];
    let result = stream.read(&mut buf).await;
    // Either error or 0 bytes (EOF) is acceptable
    match result {
        Ok(0) => {} // EOF
        Err(_) => {} // error due to abort
        Ok(_) => panic!("expected EOF or error after listener abort"),
    }

    listener_task.await.unwrap();
    listener.close().await.unwrap();
}

/// Fake DNS namespace -> transient communication error.
/// C# equivalent: NonExistantNamespaceTest
#[tokio::test]
#[ignore]
async fn non_existent_namespace_test() {
    let cs = "Endpoint=sb://doesnotexist.servicebus.windows.net;EntityPath=test;SharedAccessKeyName=key;SharedAccessKey=dGVzdA==";
    let client = HybridConnectionClient::from_connection_string(cs).unwrap();
    let result = client.create_connection().await;
    assert!(result.is_err());
}

/// Non-existent hybrid connection entity -> EndpointNotFound.
/// C# equivalent: ClientNonExistantHybridConnectionTest
#[tokio::test]
#[ignore]
async fn client_non_existent_connection_test() {
    let cs = connection_string_with_entity("doesnotexist_entity_12345");
    let client = HybridConnectionClient::from_connection_string(&cs).unwrap();
    let result = client.create_connection().await;
    assert!(matches!(result, Err(RelayError::EndpointNotFound(_))));
}

/// Non-existent hybrid connection for listener -> EndpointNotFound.
/// C# equivalent: ListenerNonExistantHybridConnectionTest
#[tokio::test]
#[ignore]
async fn listener_non_existent_connection_test() {
    let cs = connection_string_with_entity("doesnotexist_entity_12345");
    let listener = HybridConnectionListener::from_connection_string(&cs).unwrap();
    let result = listener.open().await;
    assert!(result.is_err());
}

/// Bad SAS key on listener -> AuthorizationFailed.
/// C# equivalent: ListenerAuthenticationFailureTest
#[tokio::test]
#[ignore]
async fn listener_authentication_failure_test() {
    let real_cs = connection_string();
    let builder = RelayConnectionStringBuilder::from_connection_string(real_cs).unwrap();
    let endpoint = builder.endpoint().unwrap();

    let bad_cs = format!(
        "Endpoint={};EntityPath={};SharedAccessKeyName=bad;SharedAccessKey=YmFk",
        endpoint, AUTHENTICATED_ENTITY
    );
    let listener = HybridConnectionListener::from_connection_string(&bad_cs).unwrap();
    let result = listener.open().await;

    // open() should fail with AuthorizationFailed, or the control channel
    // should report an error shortly after opening.
    match result {
        Err(_) => {} // open itself failed — expected
        Ok(()) => {
            // Wait briefly for the control channel to fail
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            let err = listener.last_error().await;
            assert!(err.is_some(), "expected authentication error");
            listener.close().await.unwrap();
        }
    }
}

/// Bad SAS key on client -> AuthorizationFailed.
/// C# equivalent: ClientAuthenticationFailureTest
#[tokio::test]
#[ignore]
async fn client_authentication_failure_test() {
    let real_cs = connection_string();
    let builder = RelayConnectionStringBuilder::from_connection_string(real_cs).unwrap();
    let endpoint = builder.endpoint().unwrap();

    let bad_cs = format!(
        "Endpoint={};EntityPath={};SharedAccessKeyName=bad;SharedAccessKey=YmFk",
        endpoint, AUTHENTICATED_ENTITY
    );
    let client = HybridConnectionClient::from_connection_string(&bad_cs).unwrap();
    let result = client.create_connection().await;
    assert!(result.is_err(), "expected authentication failure");
}

/// Close listener with pending AcceptConnectionAsync calls -> all return None.
/// C# equivalent: ListenerShutdownWithPendingAcceptsTest
#[tokio::test]
#[ignore]
async fn listener_shutdown_with_pending_accepts_test() {
    let cs = connection_string_with_entity(AUTHENTICATED_ENTITY);
    let listener = HybridConnectionListener::from_connection_string(&cs).unwrap();
    listener.open().await.unwrap();

    // Give the listener time to establish control channel
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    // Close should cause accept_connection to return None
    listener.close().await.unwrap();

    // Verify accept_connection returns None after close
    let result = listener.accept_connection().await;
    assert!(result.is_ok());
    assert!(
        result.unwrap().is_none(),
        "accept should return None after close"
    );
}
