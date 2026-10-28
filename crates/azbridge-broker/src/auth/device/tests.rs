use super::*;
use axum::{body::Body, http::Request};
use clap::Parser;
use serde_json::{Value, json};
use tower::ServiceExt;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{body_string_contains, method, path},
};

async fn setup() -> (AppState, MockServer) {
    let server = MockServer::start().await;
    let mut settings =
        Settings::from_args(crate::settings::Args::try_parse_from(["broker", "--dev"]).unwrap())
            .unwrap();
    settings.public_url = Url::parse("https://broker.test/").unwrap();
    let mut state = AppState::new(settings).unwrap();
    let mut auth = AuthService::from_settings(&state.settings).unwrap();
    auth.github = Some(Arc::new(
        GitHubClient::with_endpoints(
            "client-id".into(),
            "test-secret".into(),
            Url::parse(&format!("{}/", server.uri())).unwrap(),
            Url::parse(&format!("{}/", server.uri())).unwrap(),
        )
        .unwrap(),
    ));
    state.auth = Arc::new(auth);
    Mock::given(method("POST"))
        .and(path("/login/device/code"))
        .and(body_string_contains("client_id=client-id"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "device_code":"private-device-code", "user_code":"ABCD-EFGH",
            "verification_uri":"https://github.com/login/device", "expires_in":900, "interval":5
        })))
        .mount(&server)
        .await;
    (state, server)
}

async fn send(state: &AppState, endpoint: &str, headers: HeaderMap) -> Response {
    let mut request = Request::builder().method("POST").uri(endpoint);
    *request.headers_mut().unwrap() = headers;
    crate::app(state.clone())
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

async fn body(response: Response) -> Value {
    serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), 65536)
            .await
            .unwrap(),
    )
    .unwrap()
}

async fn begin(state: &AppState) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert("x-csrf-token", "device-login".parse().unwrap());
    headers.insert(header::ORIGIN, "https://broker.test".parse().unwrap());
    let response = send(state, "/auth/github/device/start", headers.clone()).await;
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    let cookie = response.headers()[header::SET_COOKIE]
        .to_str()
        .unwrap()
        .to_owned();
    for flag in [
        "HttpOnly",
        "Secure",
        "SameSite=Lax",
        "Path=/auth/github/device",
    ] {
        assert!(cookie.contains(flag));
    }
    let payload = body(response).await;
    assert_eq!(payload["userCode"], "ABCD-EFGH");
    assert_eq!(payload["interval"], 5);
    assert_eq!(payload["expiresIn"], 900);
    assert_eq!(
        payload["verificationUri"],
        "https://github.com/login/device"
    );
    assert!(!payload.to_string().contains("private-device-code"));
    assert!(payload.get("deviceCode").is_none());
    headers.insert(
        header::COOKIE,
        cookie.split(';').next().unwrap().parse().unwrap(),
    );
    headers.insert(
        "x-csrf-token",
        payload["csrfToken"].as_str().unwrap().parse().unwrap(),
    );
    headers
}

fn challenge(state: &AppState, headers: &HeaderMap) -> DeviceChallenge {
    state
        .auth
        .codec
        .open(cookie(headers, DEVICE_COOKIE).unwrap())
        .unwrap()
}

async fn ready(state: &AppState, headers: &HeaderMap) -> String {
    let key = format!("device-login:{}", challenge(state, headers).id);
    let entity = state
        .metadata_store
        .get(Table::Enrollments, PLATFORM_PARTITION, &key)
        .await
        .unwrap()
        .unwrap();
    assert!(!entity.data.to_string().contains("private-device-code"));
    let mut record: DeviceLoginRecord = serde_json::from_value(entity.data).unwrap();
    record.next_poll_at = 0;
    state
        .metadata_store
        .replace(
            Table::Enrollments,
            PLATFORM_PARTITION,
            &key,
            serde_json::to_value(record).unwrap(),
            &entity.etag,
        )
        .await
        .unwrap();
    key
}

async fn token_reply(server: &MockServer, value: Value) {
    Mock::given(method("POST"))
        .and(path("/login/oauth/access_token"))
        .and(body_string_contains("device_code=private-device-code"))
        .and(body_string_contains(
            "grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Adevice_code",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(value))
        .expect(1)
        .mount(server)
        .await;
}

async fn identity(server: &MockServer, id: u64, status: u16) {
    Mock::given(method("GET"))
        .and(path("/user"))
        .respond_with(ResponseTemplate::new(status).set_body_json(json!({
            "id":id, "login":"test-user", "avatar_url":""
        })))
        .expect(1)
        .mount(server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/applications/client-id/token"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(server)
        .await;
}

#[tokio::test]
async fn browser_device_start_and_poll_reject_cross_site_or_unbound_requests() {
    let (state, server) = setup().await;
    assert_eq!(
        send(&state, "/auth/github/device/start", HeaderMap::new())
            .await
            .status(),
        403
    );
    let mut evil = HeaderMap::new();
    evil.insert("x-csrf-token", "device-login".parse().unwrap());
    evil.insert(header::ORIGIN, "https://evil.test".parse().unwrap());
    assert_eq!(
        send(&state, "/auth/github/device/start", evil)
            .await
            .status(),
        403
    );
    let headers = begin(&state).await;
    let mut altered = headers.clone();
    altered.insert("x-csrf-token", "wrong".parse().unwrap());
    assert_eq!(
        send(&state, "/auth/github/device/poll", altered)
            .await
            .status(),
        403
    );
    let mut altered = headers.clone();
    altered.remove("x-csrf-token");
    assert_eq!(
        send(&state, "/auth/github/device/poll", altered)
            .await
            .status(),
        403
    );
    let mut altered = headers.clone();
    altered.insert(header::ORIGIN, "https://evil.test".parse().unwrap());
    assert_eq!(
        send(&state, "/auth/github/device/poll", altered)
            .await
            .status(),
        403
    );
    let mut altered = headers.clone();
    altered.remove(header::COOKIE);
    assert_eq!(
        send(&state, "/auth/github/device/poll", altered)
            .await
            .status(),
        401
    );
    let mut altered = headers.clone();
    altered.insert(header::COOKIE, "azbridge_device=tampered".parse().unwrap());
    assert_eq!(
        send(&state, "/auth/github/device/poll", altered)
            .await
            .status(),
        401
    );
    let other_browser = begin(&state).await;
    let mut altered = other_browser;
    altered.insert("x-csrf-token", headers["x-csrf-token"].clone());
    assert_eq!(
        send(&state, "/auth/github/device/poll", altered)
            .await
            .status(),
        403
    );
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
}

#[tokio::test]
async fn browser_device_early_poll_slows_down_without_contacting_github() {
    let (state, server) = setup().await;
    let headers = begin(&state).await;
    let response = send(&state, "/auth/github/device/poll", headers).await;
    assert_eq!(response.status(), 200);
    assert!(response.headers().get(header::SET_COOKIE).is_none());
    assert_eq!(
        body(response).await,
        json!({"status":"slowDown","interval":10})
    );
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn browser_device_pending_and_slowdown_preserve_server_poll_interval() {
    for (error, status, interval) in [
        ("authorization_pending", "pending", 5),
        ("slow_down", "slowDown", 10),
    ] {
        let (state, server) = setup().await;
        let headers = begin(&state).await;
        let key = ready(&state, &headers).await;
        token_reply(&server, json!({"error":error})).await;
        let response = send(&state, "/auth/github/device/poll", headers).await;
        assert_eq!(response.status(), 200);
        assert!(response.headers().get(header::SET_COOKIE).is_none());
        assert_eq!(
            body(response).await,
            json!({"status":status,"interval":interval})
        );
        let record = state
            .metadata_store
            .get(Table::Enrollments, PLATFORM_PARTITION, &key)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(record.data["interval"], interval);
        assert!(record.data["nextPollAt"].as_u64().unwrap() > now().unwrap());
    }
}

#[tokio::test]
async fn browser_device_denied_and_expired_consume_challenge_without_session() {
    for (error, status) in [("access_denied", "denied"), ("expired_token", "expired")] {
        let (state, server) = setup().await;
        let headers = begin(&state).await;
        let key = ready(&state, &headers).await;
        token_reply(&server, json!({"error":error})).await;
        let response = send(&state, "/auth/github/device/poll", headers).await;
        assert_eq!(response.status(), 200);
        let cookies: Vec<_> = response
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .collect();
        assert_eq!(cookies.len(), 1);
        assert!(cookies[0].to_str().unwrap().contains("Max-Age=0"));
        assert_eq!(body(response).await, json!({"status":status}));
        assert!(
            state
                .metadata_store
                .get(Table::Enrollments, PLATFORM_PARTITION, &key)
                .await
                .unwrap()
                .is_none()
        );
    }
}

#[tokio::test]
async fn browser_device_success_sets_normal_session_revokes_token_and_rejects_replay() {
    let (state, server) = setup().await;
    let headers = begin(&state).await;
    let key = ready(&state, &headers).await;
    token_reply(&server, json!({"access_token":"test-user-token"})).await;
    identity(&server, 1, 200).await;
    let response = send(&state, "/auth/github/device/poll", headers.clone()).await;
    assert_eq!(response.status(), 200);
    let session_cookie = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .find(|value| value.to_str().unwrap().starts_with("azbridge_session="))
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    assert_eq!(body(response).await, json!({"status":"authorized"}));
    let mut session_headers = HeaderMap::new();
    session_headers.insert(header::COOKIE, session_cookie.parse().unwrap());
    assert_eq!(
        current_session(&state, &session_headers).unwrap().user.id,
        1
    );
    assert!(
        state
            .metadata_store
            .get(Table::Enrollments, PLATFORM_PARTITION, &key)
            .await
            .unwrap()
            .is_none()
    );
    let replay = send(&state, "/auth/github/device/poll", headers).await;
    assert_eq!(body(replay).await, json!({"status":"expired"}));
}

#[tokio::test]
async fn browser_device_unlisted_identity_is_denied_and_lookup_failure_never_sets_session() {
    for (id, upstream, expected) in [(2, 200, 200), (1, 500, 503)] {
        let (state, server) = setup().await;
        let headers = begin(&state).await;
        ready(&state, &headers).await;
        token_reply(&server, json!({"access_token":"test-user-token"})).await;
        identity(&server, id, upstream).await;
        let response = send(&state, "/auth/github/device/poll", headers).await;
        assert_eq!(response.status(), expected);
        assert!(
            !response
                .headers()
                .get_all(header::SET_COOKIE)
                .iter()
                .any(|cookie| cookie.to_str().unwrap().starts_with("azbridge_session="))
        );
        if upstream == 200 {
            assert_eq!(body(response).await, json!({"status":"denied"}));
        }
    }
}

#[tokio::test]
async fn browser_device_expiry_is_enforced_locally_and_records_roundtrip() {
    let (state, server) = setup().await;
    let headers = begin(&state).await;
    let key = ready(&state, &headers).await;
    let entity = state
        .metadata_store
        .get(Table::Enrollments, PLATFORM_PARTITION, &key)
        .await
        .unwrap()
        .unwrap();
    let mut record: DeviceLoginRecord = serde_json::from_value(entity.data).unwrap();
    let encoded = serde_json::to_value(&record).unwrap();
    let decoded: DeviceLoginRecord = serde_json::from_value(encoded.clone()).unwrap();
    assert_eq!(serde_json::to_value(decoded).unwrap(), encoded);
    record.expires_at = 0;
    state
        .metadata_store
        .replace(
            Table::Enrollments,
            PLATFORM_PARTITION,
            &key,
            serde_json::to_value(record).unwrap(),
            &entity.etag,
        )
        .await
        .unwrap();
    let response = send(&state, "/auth/github/device/poll", headers).await;
    assert_eq!(body(response).await, json!({"status":"expired"}));
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn browser_device_unconfigured_and_upstream_failures_are_explicit() {
    let (mut state, server) = setup().await;
    server.reset().await;
    Mock::given(method("POST"))
        .and(path("/login/device/code"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    let mut headers = HeaderMap::new();
    headers.insert("x-csrf-token", "device-login".parse().unwrap());
    assert_eq!(
        send(&state, "/auth/github/device/start", headers.clone())
            .await
            .status(),
        503
    );
    state.auth = Arc::new(AuthService::from_settings(&state.settings).unwrap());
    assert_eq!(
        send(&state, "/auth/github/device/start", headers)
            .await
            .status(),
        404
    );
}

#[tokio::test]
async fn browser_device_cas_conflict_never_creates_a_session() {
    let (state, server) = setup().await;
    let headers = begin(&state).await;
    let key = ready(&state, &headers).await;
    Mock::given(method("POST"))
        .and(path("/login/oauth/access_token"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"access_token":"test-user-token"}))
                .set_delay(Duration::from_millis(300)),
        )
        .expect(1)
        .mount(&server)
        .await;
    identity(&server, 1, 200).await;
    let task_state = state.clone();
    let pending =
        tokio::spawn(async move { send(&task_state, "/auth/github/device/poll", headers).await });
    let mut started = false;
    for _ in 0..100 {
        if server
            .received_requests()
            .await
            .unwrap()
            .iter()
            .any(|request| request.url.path() == "/login/oauth/access_token")
        {
            started = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    assert!(started);
    let entity = state
        .metadata_store
        .get(Table::Enrollments, PLATFORM_PARTITION, &key)
        .await
        .unwrap()
        .unwrap();
    state
        .metadata_store
        .replace(
            Table::Enrollments,
            PLATFORM_PARTITION,
            &key,
            entity.data,
            &entity.etag,
        )
        .await
        .unwrap();
    let response = pending.await.unwrap();
    assert_eq!(response.status(), 409);
    assert!(response.headers().get(header::SET_COOKIE).is_none());
}

#[tokio::test]
async fn browser_device_workspace_member_can_sign_in_without_platform_admin() {
    let (state, server) = setup().await;
    let ws = uuid::Uuid::new_v4().to_string();
    state
        .metadata_store
        .insert(
            Table::Members,
            &ws,
            "2",
            json!({
                "user":{"id":2,"login":"test-user","avatarUrl":""},"role":"reader"
            }),
        )
        .await
        .unwrap();
    state
        .metadata_store
        .insert(Table::UserWorkspaces, "2", &ws, json!({"workspaceId":ws}))
        .await
        .unwrap();
    state.metadata_store.insert(Table::Workspaces, PLATFORM_PARTITION, &ws, json!({
        "workspace":{"id":ws,"name":"Test","namespaceName":"azb-test","location":"uksouth","state":"ready"},
        "createdBy":1
    })).await.unwrap();
    let headers = begin(&state).await;
    ready(&state, &headers).await;
    token_reply(&server, json!({"access_token":"test-user-token"})).await;
    identity(&server, 2, 200).await;
    let response = send(&state, "/auth/github/device/poll", headers).await;
    assert_eq!(response.status(), 200);
    assert!(
        response
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .any(|value| value.to_str().unwrap().starts_with("azbridge_session="))
    );
    assert_eq!(body(response).await, json!({"status":"authorized"}));
}
