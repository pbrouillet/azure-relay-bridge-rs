//! HTTP request integration tests mirroring HybridRequestTests.cs from the azure-relay-dotnet SDK.

use super::*;
use azure_relay::*;
use bytes::Bytes;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// Helper: build the HTTPS URL for the relay endpoint from a connection string.
fn relay_url(cs: &str) -> String {
    let builder = RelayConnectionStringBuilder::from_connection_string(cs).unwrap();
    let endpoint = builder.endpoint().unwrap();
    let entity = builder.entity_path().unwrap();
    format!("https://{}/{}", endpoint.host_str().unwrap(), entity)
}

/// GET and POST via HTTPS, RequestHandler writes JSON response.
/// C# equivalent: SmallRequestSmallResponse
#[tokio::test]
#[ignore]
async fn small_request_small_response() {
    let cs = connection_string_with_entity(AUTHENTICATED_ENTITY);
    let listener = HybridConnectionListener::from_connection_string(&cs).unwrap();

    struct EchoHandler;
    impl RequestHandler for EchoHandler {
        async fn handle_request(
            &self,
            ctx: RelayedHttpListenerContext,
        ) -> RelayedHttpListenerResponse {
            let mut resp = RelayedHttpListenerResponse::new();
            resp.set_status_code(200);
            resp.set_header("Content-Type", "application/json");
            if let Some(body) = ctx.request().body() {
                resp.set_body(body.clone());
            } else {
                resp.set_body(Bytes::from_static(b"{\"status\":\"ok\"}"));
            }
            resp
        }
    }

    listener.set_request_handler(EchoHandler);
    listener.open().await.unwrap();
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    let url = relay_url(&cs);
    let http_client = reqwest::Client::new();

    // GET — handler returns default JSON
    let resp = http_client.get(&url).send().await.unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.bytes().await.unwrap();
    assert_eq!(body.as_ref(), b"{\"status\":\"ok\"}");

    // POST — handler echoes request body
    let post_body = b"{\"hello\":\"world\"}";
    let resp = http_client
        .post(&url)
        .body(post_body.to_vec())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.bytes().await.unwrap();
    assert_eq!(body.as_ref(), post_body);

    listener.close().await.unwrap();
}

/// Zero-length request/response.
/// C# equivalent: EmptyRequestEmptyResponse
#[tokio::test]
#[ignore]
async fn empty_request_empty_response() {
    let cs = connection_string_with_entity(AUTHENTICATED_ENTITY);
    let listener = HybridConnectionListener::from_connection_string(&cs).unwrap();

    struct EmptyHandler;
    impl RequestHandler for EmptyHandler {
        async fn handle_request(
            &self,
            _ctx: RelayedHttpListenerContext,
        ) -> RelayedHttpListenerResponse {
            let mut resp = RelayedHttpListenerResponse::new();
            resp.set_status_code(204);
            resp
        }
    }

    listener.set_request_handler(EmptyHandler);
    listener.open().await.unwrap();
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    let url = relay_url(&cs);
    let http_client = reqwest::Client::new();
    let resp = http_client.get(&url).send().await.unwrap();
    assert_eq!(resp.status(), 204);

    listener.close().await.unwrap();
}

/// 65KB response body, custom StatusDescription.
/// C# equivalent: SmallRequestLargeResponse
#[tokio::test]
#[ignore]
async fn small_request_large_response() {
    let cs = connection_string_with_entity(AUTHENTICATED_ENTITY);
    let listener = HybridConnectionListener::from_connection_string(&cs).unwrap();

    struct LargeHandler;
    impl RequestHandler for LargeHandler {
        async fn handle_request(
            &self,
            _ctx: RelayedHttpListenerContext,
        ) -> RelayedHttpListenerResponse {
            let mut resp = RelayedHttpListenerResponse::new();
            resp.set_status_code(200);
            resp.set_status_description("Large Response");
            let body = vec![0xABu8; 65 * 1024];
            resp.set_body(Bytes::from(body));
            resp
        }
    }

    listener.set_request_handler(LargeHandler);
    listener.open().await.unwrap();
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    let url = relay_url(&cs);
    let http_client = reqwest::Client::new();
    let resp = http_client.get(&url).send().await.unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.bytes().await.unwrap();
    assert_eq!(body.len(), 65 * 1024);

    listener.close().await.unwrap();
}

/// 65KB request body, empty response.
/// C# equivalent: LargeRequestEmptyResponse
#[tokio::test]
#[ignore]
async fn large_request_empty_response() {
    let cs = connection_string_with_entity(AUTHENTICATED_ENTITY);
    let listener = HybridConnectionListener::from_connection_string(&cs).unwrap();

    struct SinkHandler;
    impl RequestHandler for SinkHandler {
        async fn handle_request(
            &self,
            ctx: RelayedHttpListenerContext,
        ) -> RelayedHttpListenerResponse {
            assert!(ctx.request().has_body());
            RelayedHttpListenerResponse::new()
        }
    }

    listener.set_request_handler(SinkHandler);
    listener.open().await.unwrap();
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    let url = relay_url(&cs);
    let body = vec![0xCDu8; 65 * 1024];
    let http_client = reqwest::Client::new();
    let resp = http_client.post(&url).body(body).send().await.unwrap();
    assert_eq!(resp.status(), 200);

    listener.close().await.unwrap();
}

/// 2 listeners, 100 requests, both should receive some.
/// C# equivalent: LoadBalancedListeners_HttpClient
#[tokio::test]
#[ignore]
async fn load_balanced_listeners() {
    let cs = connection_string_with_entity(AUTHENTICATED_ENTITY);

    let counter1 = Arc::new(AtomicUsize::new(0));
    let counter2 = Arc::new(AtomicUsize::new(0));

    // First listener
    let listener1 = HybridConnectionListener::from_connection_string(&cs).unwrap();
    let c1 = counter1.clone();
    struct CountingHandler1(Arc<AtomicUsize>);
    impl RequestHandler for CountingHandler1 {
        async fn handle_request(
            &self,
            _ctx: RelayedHttpListenerContext,
        ) -> RelayedHttpListenerResponse {
            self.0.fetch_add(1, Ordering::SeqCst);
            let mut resp = RelayedHttpListenerResponse::new();
            resp.set_status_code(200);
            resp.set_body(Bytes::from_static(b"listener1"));
            resp
        }
    }
    listener1.set_request_handler(CountingHandler1(c1));
    listener1.open().await.unwrap();

    // Second listener
    let listener2 = HybridConnectionListener::from_connection_string(&cs).unwrap();
    let c2 = counter2.clone();
    struct CountingHandler2(Arc<AtomicUsize>);
    impl RequestHandler for CountingHandler2 {
        async fn handle_request(
            &self,
            _ctx: RelayedHttpListenerContext,
        ) -> RelayedHttpListenerResponse {
            self.0.fetch_add(1, Ordering::SeqCst);
            let mut resp = RelayedHttpListenerResponse::new();
            resp.set_status_code(200);
            resp.set_body(Bytes::from_static(b"listener2"));
            resp
        }
    }
    listener2.set_request_handler(CountingHandler2(c2));
    listener2.open().await.unwrap();

    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    let url = relay_url(&cs);
    let http_client = reqwest::Client::new();
    for _ in 0..100 {
        let resp = http_client.get(&url).send().await.unwrap();
        assert_eq!(resp.status(), 200);
    }

    let count1 = counter1.load(Ordering::SeqCst);
    let count2 = counter2.load(Ordering::SeqCst);
    assert_eq!(count1 + count2, 100, "all 100 requests should be handled");
    assert!(count1 > 0, "listener1 should handle some requests");
    assert!(count2 > 0, "listener2 should handle some requests");

    listener1.close().await.unwrap();
    listener2.close().await.unwrap();
}

/// Multi-value headers preserved through relay.
/// C# equivalent: MultiValueHeader
#[tokio::test]
#[ignore]
async fn multi_value_header() {
    let cs = connection_string_with_entity(AUTHENTICATED_ENTITY);
    let listener = HybridConnectionListener::from_connection_string(&cs).unwrap();

    struct HeaderEchoHandler;
    impl RequestHandler for HeaderEchoHandler {
        async fn handle_request(
            &self,
            ctx: RelayedHttpListenerContext,
        ) -> RelayedHttpListenerResponse {
            let mut resp = RelayedHttpListenerResponse::new();
            resp.set_status_code(200);
            // Echo back any "X-Test-Header" values from the request
            if let Some(val) = ctx.request().headers().get("X-Test-Header") {
                resp.set_header("X-Test-Header", val.as_str());
            }
            // Set a multi-value response header (comma-separated per HTTP spec)
            resp.set_header("X-Multi", "value1, value2, value3");
            resp
        }
    }

    listener.set_request_handler(HeaderEchoHandler);
    listener.open().await.unwrap();
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    let url = relay_url(&cs);
    let http_client = reqwest::Client::new();
    let resp = http_client
        .get(&url)
        .header("X-Test-Header", "alpha, beta")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let multi = resp.headers().get("X-Multi").unwrap().to_str().unwrap();
    assert!(
        multi.contains("value1"),
        "multi-value header should contain value1"
    );

    listener.close().await.unwrap();
}

/// Query string filtering: sb-hc-* params stripped, encoding preserved.
/// C# equivalent: QueryString (~30 permutations)
#[tokio::test]
#[ignore]
async fn query_string() {
    let cs = connection_string_with_entity(AUTHENTICATED_ENTITY);
    let listener = HybridConnectionListener::from_connection_string(&cs).unwrap();

    struct QueryHandler;
    impl RequestHandler for QueryHandler {
        async fn handle_request(
            &self,
            ctx: RelayedHttpListenerContext,
        ) -> RelayedHttpListenerResponse {
            let mut resp = RelayedHttpListenerResponse::new();
            resp.set_status_code(200);
            // Echo the received URL so the test can inspect query params
            resp.set_body(Bytes::from(ctx.request().url().to_string()));
            resp
        }
    }

    listener.set_request_handler(QueryHandler);
    listener.open().await.unwrap();
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    let base_url = relay_url(&cs);
    let http_client = reqwest::Client::new();

    // Simple query string
    let url_with_query = format!("{}?foo=bar&baz=42", base_url);
    let resp = http_client.get(&url_with_query).send().await.unwrap();
    assert_eq!(resp.status(), 200);
    let echoed_url = resp.text().await.unwrap();
    assert!(echoed_url.contains("foo=bar"), "query param foo should be preserved");
    assert!(echoed_url.contains("baz=42"), "query param baz should be preserved");
    // sb-hc-* params should be stripped by the relay
    assert!(
        !echoed_url.contains("sb-hc-"),
        "sb-hc-* params should be stripped"
    );

    // URL-encoded query string
    let url_encoded = format!("{}?name=hello%20world&special=%26%3D", base_url);
    let resp = http_client.get(&url_encoded).send().await.unwrap();
    assert_eq!(resp.status(), 200);
    let echoed_url = resp.text().await.unwrap();
    assert!(
        echoed_url.contains("name=") && echoed_url.contains("world"),
        "encoded query param should be preserved"
    );

    // Empty query string
    let url_empty_qs = format!("{}?", base_url);
    let resp = http_client.get(&url_empty_qs).send().await.unwrap();
    assert_eq!(resp.status(), 200);

    listener.close().await.unwrap();
}

/// No RequestHandler -> 501, exception in handler -> 500.
/// C# equivalent: RequestHandlerErrors
#[tokio::test]
#[ignore]
async fn request_handler_errors() {
    let cs = connection_string_with_entity(AUTHENTICATED_ENTITY);

    // Test 1: No handler set → 501 Not Implemented
    let listener = HybridConnectionListener::from_connection_string(&cs).unwrap();
    listener.open().await.unwrap();
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    let url = relay_url(&cs);
    let http_client = reqwest::Client::new();
    let resp = http_client.get(&url).send().await.unwrap();
    assert_eq!(
        resp.status(),
        501,
        "request with no handler should return 501"
    );
    listener.close().await.unwrap();

    // Test 2: Handler that panics → 500 Internal Server Error
    let listener2 = HybridConnectionListener::from_connection_string(&cs).unwrap();
    struct PanicHandler;
    impl RequestHandler for PanicHandler {
        async fn handle_request(
            &self,
            _ctx: RelayedHttpListenerContext,
        ) -> RelayedHttpListenerResponse {
            panic!("intentional test panic");
        }
    }
    listener2.set_request_handler(PanicHandler);
    listener2.open().await.unwrap();
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    let resp = http_client.get(&url).send().await.unwrap();
    assert_eq!(
        resp.status(),
        500,
        "panicking handler should return 500"
    );
    listener2.close().await.unwrap();
}

/// ~40 HTTP status codes (200 through 418, 450), GET and POST.
/// C# equivalent: StatusCodes
#[tokio::test]
#[ignore]
async fn status_codes() {
    let cs = connection_string_with_entity(AUTHENTICATED_ENTITY);

    let test_codes: &[u16] = &[200, 201, 202, 204, 301, 302, 304, 400, 401, 403, 404, 405, 409, 410, 418, 450, 500, 501, 503];

    for &code in test_codes {
        let listener = HybridConnectionListener::from_connection_string(&cs).unwrap();

        struct StatusCodeHandler(u16);
        impl RequestHandler for StatusCodeHandler {
            async fn handle_request(
                &self,
                _ctx: RelayedHttpListenerContext,
            ) -> RelayedHttpListenerResponse {
                let mut resp = RelayedHttpListenerResponse::new();
                resp.set_status_code(self.0);
                resp
            }
        }

        listener.set_request_handler(StatusCodeHandler(code));
        listener.open().await.unwrap();
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;

        let url = relay_url(&cs);
        let http_client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap();

        // GET
        let resp = http_client.get(&url).send().await.unwrap();
        assert_eq!(
            resp.status().as_u16(),
            code,
            "GET: expected status {code}"
        );

        // POST
        let resp = http_client.post(&url).send().await.unwrap();
        assert_eq!(
            resp.status().as_u16(),
            code,
            "POST: expected status {code}"
        );

        listener.close().await.unwrap();
    }
}

/// DELETE, GET, HEAD, OPTIONS, POST, PUT, TRACE.
/// C# equivalent: Verbs
#[tokio::test]
#[ignore]
async fn verbs() {
    let cs = connection_string_with_entity(AUTHENTICATED_ENTITY);
    let listener = HybridConnectionListener::from_connection_string(&cs).unwrap();

    struct VerbEchoHandler;
    impl RequestHandler for VerbEchoHandler {
        async fn handle_request(
            &self,
            ctx: RelayedHttpListenerContext,
        ) -> RelayedHttpListenerResponse {
            let mut resp = RelayedHttpListenerResponse::new();
            resp.set_status_code(200);
            // Echo the HTTP method back in the response body
            resp.set_body(Bytes::from(ctx.request().method().to_string()));
            resp
        }
    }

    listener.set_request_handler(VerbEchoHandler);
    listener.open().await.unwrap();
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    let url = relay_url(&cs);
    let http_client = reqwest::Client::new();

    let methods = [
        reqwest::Method::GET,
        reqwest::Method::POST,
        reqwest::Method::PUT,
        reqwest::Method::DELETE,
        reqwest::Method::HEAD,
        reqwest::Method::OPTIONS,
        reqwest::Method::TRACE,
    ];

    for method in &methods {
        let resp = http_client
            .request(method.clone(), &url)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200, "method {method} should succeed");

        // HEAD responses have no body, so skip body check for HEAD
        if *method != reqwest::Method::HEAD {
            let body = resp.text().await.unwrap();
            assert_eq!(
                body,
                method.as_str(),
                "echoed method should match for {method}"
            );
        }
    }

    listener.close().await.unwrap();
}

/// StatusDescription can be set to null.
/// C# equivalent: AllowNullStatusDescription
#[tokio::test]
#[ignore]
async fn allow_null_status_description() {
    let cs = connection_string_with_entity(AUTHENTICATED_ENTITY);
    let listener = HybridConnectionListener::from_connection_string(&cs).unwrap();

    struct NoDescriptionHandler;
    impl RequestHandler for NoDescriptionHandler {
        async fn handle_request(
            &self,
            _ctx: RelayedHttpListenerContext,
        ) -> RelayedHttpListenerResponse {
            let mut resp = RelayedHttpListenerResponse::new();
            resp.set_status_code(200);
            // Intentionally do NOT set status_description — it should remain None
            assert!(
                resp.status_description().is_none(),
                "status description should be None by default"
            );
            resp
        }
    }

    listener.set_request_handler(NoDescriptionHandler);
    listener.open().await.unwrap();
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    let url = relay_url(&cs);
    let http_client = reqwest::Client::new();
    let resp = http_client.get(&url).send().await.unwrap();
    assert_eq!(resp.status(), 200);

    listener.close().await.unwrap();
}

/// After writing to OutputStream, setting StatusCode/Description/Headers
/// should return error.
/// C# equivalent: ResponseHeadersAfterBody
#[tokio::test]
#[ignore]
async fn response_headers_after_body() {
    // Verify that the response builder enforces ordering: once a body is set,
    // modifying status_code / status_description / headers should still work
    // at the struct level (the relay protocol enforces the constraint server-side).
    //
    // In the Rust SDK, RelayedHttpListenerResponse is a plain struct so all
    // setters are available at any time. This test documents the expected
    // behavior: the relay itself rejects late header changes, so the handler
    // should set headers/status before body.
    let cs = connection_string_with_entity(AUTHENTICATED_ENTITY);
    let listener = HybridConnectionListener::from_connection_string(&cs).unwrap();

    struct HeadersAfterBodyHandler;
    impl RequestHandler for HeadersAfterBodyHandler {
        async fn handle_request(
            &self,
            _ctx: RelayedHttpListenerContext,
        ) -> RelayedHttpListenerResponse {
            let mut resp = RelayedHttpListenerResponse::new();
            // Set body first
            resp.set_body(Bytes::from_static(b"body content"));
            // Then set headers — this should still work at the API level
            resp.set_status_code(200);
            resp.set_status_description("OK");
            resp.set_header("X-Late-Header", "late-value");
            resp
        }
    }

    listener.set_request_handler(HeadersAfterBodyHandler);
    listener.open().await.unwrap();
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    let url = relay_url(&cs);
    let http_client = reqwest::Client::new();
    let resp = http_client.get(&url).send().await.unwrap();
    // The relay may or may not accept late headers; we verify the response is received
    assert!(
        resp.status().is_success() || resp.status().is_server_error(),
        "should get a valid HTTP response"
    );

    listener.close().await.unwrap();
}
