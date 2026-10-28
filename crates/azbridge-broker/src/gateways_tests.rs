use super::*;
use std::sync::Arc;

use axum::{
    body::{Body, to_bytes},
    http::Request,
};
use azbridge_broker_api::{Connection, Member, Profile, Workspace};
use clap::Parser;
use hmac::{Hmac, Mac};
use serde_json::{Value, json};
use tower::ServiceExt;
use wiremock::{
    Mock, MockServer, ResponseTemplate,
    matchers::{method, path},
};

use crate::{
    auth::Session,
    models::{RevisionRecord, revision_key},
    settings::{Args, Settings},
};

const WS: &str = "00000000-0000-0000-0000-000000000010";
const OTHER: &str = "00000000-0000-0000-0000-000000000011";
const PROFILE: &str = "00000000-0000-0000-0000-000000000020";
const YAML: &str = "LocalForward:\n- RelayName: send\n  BindPort: 8080\nRemoteForward:\n- RelayName: listen\n  Host: localhost\n  HostPort: 8081\n";

fn user(id: u64) -> GitHubUser {
    GitHubUser {
        id,
        login: format!("user-{id}"),
        avatar_url: String::new(),
    }
}

async fn seed(state: &AppState, ws: &str) {
    state
        .metadata_store
        .insert(
            Table::Workspaces,
            PLATFORM_PARTITION,
            ws,
            encode(WorkspaceRecord {
                workspace: Workspace {
                    id: ws.into(),
                    name: ws.into(),
                    namespace_name: format!("azb-{ws}"),
                    location: "westus".into(),
                    state: ProvisioningState::Ready,
                    error: None,
                },
                created_by: 1,
            })
            .unwrap(),
        )
        .await
        .unwrap();
    state
        .metadata_store
        .insert(
            Table::Profiles,
            ws,
            PROFILE,
            encode(ProfileRecord {
                profile: Profile {
                    id: PROFILE.into(),
                    name: "profile".into(),
                    current_revision: Some(1),
                },
                next_revision: 2,
            })
            .unwrap(),
        )
        .await
        .unwrap();
    state
        .config_store
        .put_revision(ws, PROFILE, "1", YAML)
        .await
        .unwrap();
    state
        .metadata_store
        .insert(
            Table::RevisionMetadata,
            ws,
            &revision_key(PROFILE, 1),
            encode(RevisionRecord {
                revision: 1,
                created_at: now().unwrap(),
            })
            .unwrap(),
        )
        .await
        .unwrap();
    let ns = format!("azb-{ws}");
    state
        .relay_manager
        .ensure_namespace(&ns, "westus")
        .await
        .unwrap();
    for name in ["send", "listen", "unentitled"] {
        let connection = Connection {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.into(),
            state: ProvisioningState::Ready,
            error: None,
        };
        state
            .metadata_store
            .insert(
                Table::Connections,
                ws,
                &connection.id,
                encode(&connection).unwrap(),
            )
            .await
            .unwrap();
        state
            .relay_manager
            .ensure_connection(&ns, name)
            .await
            .unwrap();
    }
}

async fn setup() -> AppState {
    let settings = Settings::from_args(Args::try_parse_from(["broker", "--dev"]).unwrap()).unwrap();
    let state = AppState::new(settings).unwrap();
    seed(&state, WS).await;
    seed(&state, OTHER).await;
    state
}

fn browser(state: &AppState, id: u64, csrf: bool) -> HeaderMap {
    let cookie = state
        .auth
        .codec
        .seal(&Session {
            user: user(id),
            csrf_token: "csrf".into(),
            expires_at: now().unwrap() + 300,
        })
        .unwrap();
    let mut headers = HeaderMap::new();
    headers.insert(
        "cookie",
        format!("azbridge_session={cookie}").parse().unwrap(),
    );
    if csrf {
        headers.insert("x-csrf-token", "csrf".parse().unwrap());
    }
    headers
}

fn credential_headers(credential: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::AUTHORIZATION,
        format!("Bearer {credential}").parse().unwrap(),
    );
    headers
}

#[tokio::test]
async fn malformed_gateway_payloads_are_sanitized() {
    let state = setup().await;
    let created = create(&state).await;
    let credential = created["credential"].as_str().unwrap();
    for (uri, payload) in [
        (
            "/api/v1/gateway/relay-tokens",
            json!({"relayName":"send","right":"private-request-marker"}),
        ),
        (
            "/api/v1/gateway/status",
            json!({"appliedRevision":"private-request-marker","version":"v1","hostname":"host"}),
        ),
    ] {
        let (status, body) =
            call(&state, "POST", uri, credential_headers(credential), payload).await;
        assert_eq!(status, 400);
        assert_eq!(body["code"], "bad_request");
        assert!(!body.to_string().contains("private-request-marker"));
    }
    let (status, body) = call(
        &state,
        "POST",
        "/api/v1/enrollments",
        HeaderMap::new(),
        json!({"workspace":42}),
    )
    .await;
    assert_eq!(status, 400);
    assert_eq!(body["code"], "bad_request");
}

async fn call(
    state: &AppState,
    method: &str,
    uri: &str,
    headers: HeaderMap,
    body: Value,
) -> (u16, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    request.headers_mut().extend(headers);
    let response = crate::app(state.clone()).oneshot(request).await.unwrap();
    let status = response.status().as_u16();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    (
        status,
        if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap()
        },
    )
}

async fn create(state: &AppState) -> Value {
    let (status, created) = call(
        state,
        "POST",
        &format!("/api/v1/workspaces/{WS}/gateways"),
        browser(state, 1, true),
        json!({"name": "gateway", "profileId": PROFILE}),
    )
    .await;
    assert_eq!(status, 201);
    created
}

async fn enrollment(state: &AppState) -> String {
    let (status, body) = call(
        state,
        "POST",
        "/api/v1/enrollments",
        HeaderMap::new(),
        json!({"workspace": WS, "profile": "profile", "gatewayName": "device"}),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(body["interval"], 1);
    body["enrollmentId"].as_str().unwrap().to_owned()
}

async fn edit_enrollment(state: &AppState, id: &str, edit: impl FnOnce(&mut EnrollmentRecord)) {
    let entity = state
        .metadata_store
        .get(Table::Enrollments, PLATFORM_PARTITION, id)
        .await
        .unwrap()
        .unwrap();
    let mut record: EnrollmentRecord = decode(entity.data).unwrap();
    edit(&mut record);
    state
        .metadata_store
        .replace(
            Table::Enrollments,
            PLATFORM_PARTITION,
            id,
            encode(record).unwrap(),
            &entity.etag,
        )
        .await
        .unwrap();
}

async fn poll(state: &AppState, id: &str) -> (u16, Value) {
    call(
        state,
        "POST",
        &format!("/api/v1/enrollments/{id}/poll"),
        HeaderMap::new(),
        Value::Null,
    )
    .await
}

async fn role(state: &AppState, id: u64, role: Role) {
    state
        .metadata_store
        .insert(
            Table::Members,
            WS,
            &id.to_string(),
            encode(Member {
                user: user(id),
                role,
            })
            .unwrap(),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn browser_create_list_revoke_hash_only_and_csrf_roles_tenant_isolation() {
    let state = setup().await;
    role(&state, 2, Role::Reader).await;
    role(&state, 3, Role::Operator).await;
    let uri = format!("/api/v1/workspaces/{WS}/gateways");
    for (headers, expected) in [
        (HeaderMap::new(), 401),
        (browser(&state, 1, false), 403),
        (browser(&state, 2, true), 403),
        (browser(&state, 4, true), 403),
    ] {
        assert_eq!(
            call(
                &state,
                "POST",
                &uri,
                headers,
                json!({"name":"gw","profileId":PROFILE})
            )
            .await
            .0,
            expected
        );
    }
    let (status, body) = call(
        &state,
        "POST",
        &uri,
        browser(&state, 3, true),
        json!({"name":"gw","profileId":PROFILE}),
    )
    .await;
    assert_eq!(status, 201);
    let credential = body["credential"].as_str().unwrap();
    assert_eq!(body["workspaceId"], WS);
    let id = body["gateway"]["id"].as_str().unwrap();
    let entity = state
        .metadata_store
        .get(Table::Gateways, WS, id)
        .await
        .unwrap()
        .unwrap();
    let stored: GatewayRecord = decode(entity.data.clone()).unwrap();
    assert_eq!(stored.credential_hash, digest(credential));
    assert!(!entity.data.to_string().contains(credential));
    let (status, list) = call(&state, "GET", &uri, browser(&state, 2, false), Value::Null).await;
    assert_eq!(status, 200);
    assert!(!list.to_string().contains("credential"));
    assert_eq!(
        call(
            &state,
            "POST",
            &format!("/api/v1/workspaces/{OTHER}/gateways"),
            browser(&state, 3, true),
            json!({"name":"gw","profileId":PROFILE})
        )
        .await
        .0,
        403
    );
    assert_eq!(
        call(
            &state,
            "DELETE",
            &format!("/api/v1/workspaces/{OTHER}/gateways/{id}"),
            browser(&state, 1, true),
            Value::Null
        )
        .await
        .0,
        404
    );
    assert_eq!(
        call(
            &state,
            "DELETE",
            &format!("{uri}/{id}"),
            browser(&state, 3, true),
            Value::Null
        )
        .await
        .0,
        204
    );
    assert_eq!(
        call(
            &state,
            "GET",
            "/api/v1/gateway/config",
            credential_headers(credential),
            Value::Null
        )
        .await
        .0,
        401
    );
    let audit = state.metadata_store.list(Table::Audit, WS).await.unwrap();
    assert_eq!(audit.len(), 2);
}

#[tokio::test]
async fn config_status_logout_and_invalid_credentials() {
    let state = setup().await;
    let created = create(&state).await;
    let credential = created["credential"].as_str().unwrap();
    let (status, config) = call(
        &state,
        "GET",
        "/api/v1/gateway/config",
        credential_headers(credential),
        Value::Null,
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(config["revision"], 1);
    assert_eq!(
        config["relayEndpoint"],
        format!("sb://azb-{WS}.servicebus.windows.net/")
    );
    assert!(config["config"].get("AzureRelayEndpoint").is_none());
    for bad in [
        "garbage".to_string(),
        credential.replace(WS, OTHER),
        format!("{credential}x"),
        credential.replace("azgw.", "other."),
    ] {
        assert_eq!(
            call(
                &state,
                "GET",
                "/api/v1/gateway/config",
                credential_headers(&bad),
                Value::Null
            )
            .await
            .0,
            401
        );
    }
    for request in [
        json!({"appliedRevision":0,"version":"1","hostname":"host"}),
        json!({"appliedRevision":2,"version":"1","hostname":"host"}),
        json!({"appliedRevision":1,"version":"","hostname":"host"}),
    ] {
        assert_eq!(
            call(
                &state,
                "POST",
                "/api/v1/gateway/status",
                credential_headers(credential),
                request
            )
            .await
            .0,
            400
        );
    }
    assert_eq!(
        call(
            &state,
            "POST",
            "/api/v1/gateway/status",
            credential_headers(credential),
            json!({"appliedRevision":1,"version":"v1","hostname":"host"})
        )
        .await
        .0,
        204
    );
    let entity = state
        .metadata_store
        .get(
            Table::Gateways,
            WS,
            created["gateway"]["id"].as_str().unwrap(),
        )
        .await
        .unwrap()
        .unwrap();
    assert!(entity.data["gateway"]["lastSeenAt"].as_u64().is_some());
    assert_eq!(entity.data["gateway"]["appliedRevision"], 1);
    assert_eq!(entity.data["gateway"]["version"], "v1");
    assert_eq!(entity.data["gateway"]["hostname"], "host");
    assert_eq!(
        call(
            &state,
            "POST",
            "/api/v1/gateway/logout",
            credential_headers(credential),
            Value::Null
        )
        .await
        .0,
        204
    );
    for (method, path, body) in [
        ("GET", "/api/v1/gateway/config", Value::Null),
        (
            "POST",
            "/api/v1/gateway/status",
            json!({"appliedRevision":1,"version":"v1","hostname":"host"}),
        ),
        (
            "POST",
            "/api/v1/gateway/relay-tokens",
            json!({"relayName":"send","right":"send"}),
        ),
        ("POST", "/api/v1/gateway/logout", Value::Null),
    ] {
        assert_eq!(
            call(&state, method, path, credential_headers(credential), body)
                .await
                .0,
            401
        );
    }
}

fn check_sas(token: &str, ws: &str, name: &str, right: &str, key: &[u8]) {
    let values: std::collections::HashMap<_, _> = url::form_urlencoded::parse(
        token
            .strip_prefix("SharedAccessSignature ")
            .unwrap()
            .as_bytes(),
    )
    .into_owned()
    .collect();
    assert_eq!(
        values["sr"],
        format!("http://azb-{ws}.servicebus.windows.net/{name}/")
    );
    assert_eq!(values["skn"], format!("azbridge-{right}"));
    let audience =
        url::form_urlencoded::byte_serialize(values["sr"].as_bytes()).collect::<String>();
    use base64::engine::general_purpose::STANDARD;
    let mut mac = Hmac::<Sha256>::new_from_slice(STANDARD.encode(key).as_bytes()).unwrap();
    mac.update(format!("{audience}\n{}", values["se"]).as_bytes());
    mac.verify_slice(&STANDARD.decode(&values["sig"]).unwrap())
        .unwrap();
    assert!(values["se"].parse::<u64>().unwrap() > now().unwrap());
}

#[tokio::test]
async fn tokens_are_directional_signed_tenant_scoped_and_follow_current_profile() {
    let state = setup().await;
    let created = create(&state).await;
    let credential = created["credential"].as_str().unwrap();
    for (name, right, key) in [
        ("send", "send", b"development-only-send-key".as_slice()),
        (
            "listen",
            "listen",
            b"development-only-listen-key".as_slice(),
        ),
    ] {
        let (status, body) = call(
            &state,
            "POST",
            "/api/v1/gateway/relay-tokens",
            credential_headers(credential),
            json!({"relayName":name,"right":right}),
        )
        .await;
        assert_eq!(status, 200);
        check_sas(body["token"].as_str().unwrap(), WS, name, right, key);
        assert_eq!(
            body["expiresAt"].as_u64().unwrap().to_string(),
            url::form_urlencoded::parse(
                body["token"]
                    .as_str()
                    .unwrap()
                    .strip_prefix("SharedAccessSignature ")
                    .unwrap()
                    .as_bytes()
            )
            .find(|(key, _)| key == "se")
            .unwrap()
            .1
        );
    }
    for (name, right) in [
        ("send", "listen"),
        ("listen", "send"),
        ("unentitled", "send"),
        ("missing", "send"),
        ("../send", "send"),
    ] {
        assert_eq!(
            call(
                &state,
                "POST",
                "/api/v1/gateway/relay-tokens",
                credential_headers(credential),
                json!({"relayName":name,"right":right})
            )
            .await
            .0,
            403
        );
    }
    state
        .config_store
        .put_revision(
            WS,
            PROFILE,
            "2",
            "LocalForward:\n- RelayName: listen\n  BindPort: 8080\n",
        )
        .await
        .unwrap();
    state
        .metadata_store
        .insert(
            Table::RevisionMetadata,
            WS,
            &revision_key(PROFILE, 2),
            encode(RevisionRecord {
                revision: 2,
                created_at: now().unwrap(),
            })
            .unwrap(),
        )
        .await
        .unwrap();
    let profile = state
        .metadata_store
        .get(Table::Profiles, WS, PROFILE)
        .await
        .unwrap()
        .unwrap();
    let mut record: ProfileRecord = decode(profile.data).unwrap();
    record.profile.current_revision = Some(2);
    state
        .metadata_store
        .replace(
            Table::Profiles,
            WS,
            PROFILE,
            encode(record).unwrap(),
            &profile.etag,
        )
        .await
        .unwrap();
    assert_eq!(
        call(
            &state,
            "POST",
            "/api/v1/gateway/relay-tokens",
            credential_headers(credential),
            json!({"relayName":"send","right":"send"})
        )
        .await
        .0,
        403
    );
    assert_eq!(
        call(
            &state,
            "POST",
            "/api/v1/gateway/relay-tokens",
            credential_headers(credential),
            json!({"relayName":"listen","right":"send"})
        )
        .await
        .0,
        200
    );
    let (status, config) = call(
        &state,
        "GET",
        "/api/v1/gateway/config",
        credential_headers(credential),
        Value::Null,
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(config["revision"], 2);
    let connection = state
        .metadata_store
        .list(Table::Connections, WS)
        .await
        .unwrap()
        .into_iter()
        .find(|e| e.data["name"] == "listen")
        .unwrap();
    let mut value = connection.data.clone();
    value["state"] = json!("deleting");
    state
        .metadata_store
        .replace(
            Table::Connections,
            WS,
            &connection.row_key,
            value,
            &connection.etag,
        )
        .await
        .unwrap();
    assert_eq!(
        call(
            &state,
            "POST",
            "/api/v1/gateway/relay-tokens",
            credential_headers(credential),
            json!({"relayName":"listen","right":"send"})
        )
        .await
        .0,
        400
    );
}

#[tokio::test]
async fn dev_enrollment_encrypts_handle_slow_down_ttl_and_one_time_consumption() {
    let state = setup().await;
    let id = enrollment(&state).await;
    assert_eq!(URL_SAFE_NO_PAD.decode(&id).unwrap().len(), 16);
    let entity = state
        .metadata_store
        .get(Table::Enrollments, PLATFORM_PARTITION, &id)
        .await
        .unwrap()
        .unwrap();
    let record: EnrollmentRecord = decode(entity.data).unwrap();
    let plaintext: String = state
        .auth
        .codec
        .open(&record.encrypted_device_code)
        .unwrap();
    assert_ne!(record.encrypted_device_code, plaintext);
    assert_eq!(
        poll(&state, &id).await.1,
        json!({"status":"slowDown","interval":6})
    );
    edit_enrollment(&state, &id, |record| record.next_poll_at = 0).await;
    let (first, second) = tokio::join!(poll(&state, &id), poll(&state, &id));
    let statuses = [
        first.1["status"].as_str().unwrap(),
        second.1["status"].as_str().unwrap(),
    ];
    assert!(statuses.contains(&"authorized"));
    assert!(statuses.contains(&"expired"));
    assert_eq!(
        state
            .metadata_store
            .list(Table::Gateways, WS)
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(matches!(
        state
            .metadata_store
            .delete(Table::Enrollments, PLATFORM_PARTITION, &id, &entity.etag)
            .await,
        Err(BrokerError::NotFound)
    ));
    let unknown = poll(&state, "unknown").await;
    let expired = enrollment(&state).await;
    edit_enrollment(&state, &expired, |record| record.expires_at = 0).await;
    assert_eq!(poll(&state, &expired).await, unknown);
    assert_eq!(poll(&state, &id).await, unknown);
}

#[tokio::test]
async fn production_missing_github_never_falls_back_and_public_routes_rate_limit() {
    let mut state = setup().await;
    let id = enrollment(&state).await;
    edit_enrollment(&state, &id, |record| record.next_poll_at = 0).await;
    Arc::get_mut(&mut state.settings).unwrap().dev = false;
    assert_eq!(poll(&state, &id).await.0, 503);
    assert_eq!(
        call(
            &state,
            "POST",
            "/api/v1/enrollments",
            HeaderMap::new(),
            json!({"workspace":WS,"profile":PROFILE,"gatewayName":"gateway"})
        )
        .await
        .0,
        503
    );
    assert!(
        state
            .metadata_store
            .list(Table::Gateways, WS)
            .await
            .unwrap()
            .is_empty()
    );
    for _ in 1..240 {
        poll(&state, "unknown").await;
    }
    assert_eq!(poll(&state, "unknown").await.0, 429);
    for _ in 1..30 {
        call(
            &state,
            "POST",
            "/api/v1/enrollments",
            HeaderMap::new(),
            json!({"workspace":"missing","profile":PROFILE,"gatewayName":"gateway"}),
        )
        .await;
    }
    assert_eq!(
        call(
            &state,
            "POST",
            "/api/v1/enrollments",
            HeaderMap::new(),
            json!({"workspace":WS,"profile":PROFILE,"gatewayName":"gateway"})
        )
        .await
        .0,
        429
    );
}

async fn mock_github(state: &mut AppState) -> MockServer {
    let server = MockServer::start().await;
    let endpoint = url::Url::parse(&format!("{}/", server.uri())).unwrap();
    Arc::get_mut(&mut state.auth).unwrap().github = Some(Arc::new(
        auth::GitHubClient::with_endpoints(
            "client".into(),
            "secret".into(),
            endpoint.clone(),
            endpoint,
        )
        .unwrap(),
    ));
    Arc::get_mut(&mut state.settings).unwrap().dev = false;
    Mock::given(method("POST")).and(path("/login/device/code")).respond_with(ResponseTemplate::new(200).set_body_json(json!({
        "device_code":"github-device-secret","user_code":"ABCD-EFGH","verification_uri":"https://github.com/login/device","expires_in":900,"interval":1
    }))).mount(&server).await;
    server
}

async fn mock_reply(server: &MockServer, reply: Value) {
    Mock::given(method("POST"))
        .and(path("/login/oauth/access_token"))
        .respond_with(ResponseTemplate::new(200).set_body_json(reply))
        .mount(server)
        .await;
}

#[tokio::test]
async fn enrollment_cas_conflict_never_mints_even_after_github_authorization() {
    let mut state = setup().await;
    let server = mock_github(&mut state).await;
    Mock::given(method("POST"))
        .and(path("/login/oauth/access_token"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"access_token":"github-user-secret"}))
                .set_delay(Duration::from_millis(300)),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/user"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"id":1,"login":"admin","avatar_url":""})),
        )
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path("/applications/client/token"))
        .respond_with(ResponseTemplate::new(204))
        .expect(1)
        .mount(&server)
        .await;
    let id = enrollment(&state).await;
    edit_enrollment(&state, &id, |record| record.next_poll_at = 0).await;
    let poll_state = state.clone();
    let poll_id = id.clone();
    let pending = tokio::spawn(async move { poll(&poll_state, &poll_id).await });
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
    // A second process changed the row after this process reserved its poll.
    edit_enrollment(&state, &id, |record| record.next_poll_at += 5).await;
    assert_eq!(pending.await.unwrap().0, 409);
    assert!(
        state
            .metadata_store
            .list(Table::Gateways, WS)
            .await
            .unwrap()
            .is_empty()
    );
    server.verify().await;
}

#[tokio::test]
async fn github_pending_slow_down_expired_denied_and_safe_failure() {
    for (reply, expected) in [
        (
            json!({"error":"authorization_pending"}),
            json!({"status":"pending","interval":1}),
        ),
        (
            json!({"error":"slow_down"}),
            json!({"status":"slowDown","interval":6}),
        ),
        (
            json!({"error":"expired_token"}),
            json!({"status":"expired"}),
        ),
        (json!({"error":"access_denied"}), json!({"status":"denied"})),
    ] {
        let mut state = setup().await;
        let server = mock_github(&mut state).await;
        mock_reply(&server, reply).await;
        let id = enrollment(&state).await;
        edit_enrollment(&state, &id, |record| record.next_poll_at = 0).await;
        assert_eq!(poll(&state, &id).await, (200, expected.clone()));
        let entity = state
            .metadata_store
            .get(Table::Enrollments, PLATFORM_PARTITION, &id)
            .await
            .unwrap();
        if matches!(expected["status"].as_str().unwrap(), "expired" | "denied") {
            assert!(entity.is_none());
        }
        if expected["status"] == "slowDown" {
            let record: EnrollmentRecord = decode(entity.unwrap().data).unwrap();
            assert_eq!(record.interval, 6);
            assert!(record.next_poll_at >= now().unwrap() + 5);
        }
    }
    let mut state = setup().await;
    let server = mock_github(&mut state).await;
    Mock::given(path("/login/oauth/access_token"))
        .respond_with(ResponseTemplate::new(500).set_body_string("secret-device-code"))
        .mount(&server)
        .await;
    let id = enrollment(&state).await;
    edit_enrollment(&state, &id, |record| record.next_poll_at = 0).await;
    let (status, error) = poll(&state, &id).await;
    assert_eq!(status, 503);
    assert!(!error.to_string().contains("secret-device-code"));
}

#[tokio::test]
async fn github_identity_is_consumed_revoked_and_membership_checked_at_consumption() {
    for (id, member_role, expected) in [
        (1, None, "authorized"),
        (2, Some(Role::Reader), "denied"),
        (3, Some(Role::Operator), "authorized"),
        (4, Some(Role::Admin), "authorized"),
        (5, None, "denied"),
    ] {
        let mut state = setup().await;
        let server = mock_github(&mut state).await;
        mock_reply(&server, json!({"access_token":"github-user-secret"})).await;
        Mock::given(method("GET"))
            .and(path("/user"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"id":id,"login":"test","avatar_url":""})),
            )
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("DELETE"))
            .and(path("/applications/client/token"))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&server)
            .await;
        let enrollment = enrollment(&state).await;
        if let Some(member_role) = member_role {
            role(&state, id, member_role).await;
        }
        edit_enrollment(&state, &enrollment, |record| record.next_poll_at = 0).await;
        let (status, body) = poll(&state, &enrollment).await;
        assert_eq!(status, 200);
        assert_eq!(body["status"], expected);
        assert_eq!(poll(&state, &enrollment).await.1["status"], "expired");
        assert!(
            !serde_json::to_string(
                &state
                    .metadata_store
                    .list(Table::Gateways, WS)
                    .await
                    .unwrap()
            )
            .unwrap()
            .contains("github-user-secret")
        );
        server.verify().await;
    }
}

#[tokio::test]
async fn enrollment_requires_ready_published_profile_and_valid_names() {
    let state = setup().await;
    for request in [
        json!({"workspace":"missing","profile":PROFILE,"gatewayName":"valid"}),
        json!({"workspace":WS,"profile":"missing","gatewayName":"valid"}),
        json!({"workspace":WS,"profile":PROFILE,"gatewayName":""}),
    ] {
        assert_eq!(
            call(
                &state,
                "POST",
                "/api/v1/enrollments",
                HeaderMap::new(),
                request
            )
            .await
            .0,
            400
        );
    }
}

#[tokio::test]
async fn mutations_recheck_membership_inside_lock_and_cas_preserves_revocation() {
    let state = setup().await;
    role(&state, 2, Role::Operator).await;
    let created = create(&state).await;
    let gateway_id = created["gateway"]["id"].as_str().unwrap();
    let stale = state
        .metadata_store
        .get(Table::Gateways, WS, gateway_id)
        .await
        .unwrap()
        .unwrap();
    let mut stale_record: GatewayRecord = decode(stale.data.clone()).unwrap();
    let guard = state.mutation_lock.lock().await;
    let request_state = state.clone();
    let request_headers = browser(&state, 2, true);
    let pending = tokio::spawn(async move {
        call(
            &request_state,
            "POST",
            &format!("/api/v1/workspaces/{WS}/gateways"),
            request_headers,
            json!({"name":"after-removal","profileId":PROFILE}),
        )
        .await
    });
    tokio::task::yield_now().await;
    let member = state
        .metadata_store
        .get(Table::Members, WS, "2")
        .await
        .unwrap()
        .unwrap();
    state
        .metadata_store
        .delete(Table::Members, WS, "2", &member.etag)
        .await
        .unwrap();
    drop(guard);
    assert_eq!(pending.await.unwrap().0, 403);
    assert_eq!(
        call(
            &state,
            "DELETE",
            &format!("/api/v1/workspaces/{WS}/gateways/{gateway_id}"),
            browser(&state, 1, true),
            Value::Null
        )
        .await
        .0,
        204
    );
    stale_record.gateway.last_seen_at = Some(now().unwrap());
    assert!(matches!(
        save_gateway(&state, &stale, &stale_record).await,
        Err(BrokerError::Conflict)
    ));
    let current = state
        .metadata_store
        .get(Table::Gateways, WS, gateway_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(current.data["gateway"]["revoked"], true);
}

#[tokio::test]
async fn github_identity_or_revocation_failures_never_mint_credentials() {
    for (identity_status, revoke_status) in [(500, 204), (200, 500)] {
        let mut state = setup().await;
        let server = mock_github(&mut state).await;
        mock_reply(&server, json!({"access_token":"github-user-secret"})).await;
        Mock::given(method("GET"))
            .and(path("/user"))
            .respond_with(
                ResponseTemplate::new(identity_status)
                    .set_body_json(json!({"id":1,"login":"test","avatar_url":""})),
            )
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("DELETE"))
            .and(path("/applications/client/token"))
            .respond_with(ResponseTemplate::new(revoke_status))
            .expect(1)
            .mount(&server)
            .await;
        let id = enrollment(&state).await;
        edit_enrollment(&state, &id, |record| record.next_poll_at = 0).await;
        let (status, body) = poll(&state, &id).await;
        assert_eq!(status, 503);
        assert!(!body.to_string().contains("github-user-secret"));
        assert!(
            state
                .metadata_store
                .list(Table::Gateways, WS)
                .await
                .unwrap()
                .is_empty()
        );
        server.verify().await;
    }
}

#[tokio::test]
async fn readiness_and_profile_scope_are_revalidated_and_names_are_bounded() {
    let state = setup().await;
    let created = create(&state).await;
    let credential = created["credential"].as_str().unwrap();
    for name in [
        "".into(),
        " leading".into(),
        "line\nbreak".into(),
        "x".repeat(129),
    ] {
        assert_eq!(
            call(
                &state,
                "POST",
                &format!("/api/v1/workspaces/{WS}/gateways"),
                browser(&state, 1, true),
                json!({"name":name,"profileId":PROFILE})
            )
            .await
            .0,
            400
        );
    }
    assert_eq!(
        call(
            &state,
            "POST",
            &format!("/api/v1/workspaces/{WS}/gateways"),
            browser(&state, 1, true),
            json!({"name":"gateway","profileId":OTHER})
        )
        .await
        .0,
        404
    );
    let workspace = state
        .metadata_store
        .get(Table::Workspaces, PLATFORM_PARTITION, WS)
        .await
        .unwrap()
        .unwrap();
    let mut record: WorkspaceRecord = decode(workspace.data).unwrap();
    record.workspace.state = ProvisioningState::Provisioning;
    state
        .metadata_store
        .replace(
            Table::Workspaces,
            PLATFORM_PARTITION,
            WS,
            encode(record).unwrap(),
            &workspace.etag,
        )
        .await
        .unwrap();
    assert_eq!(
        call(
            &state,
            "GET",
            "/api/v1/gateway/config",
            credential_headers(credential),
            Value::Null
        )
        .await
        .0,
        409
    );
    assert_eq!(
        call(
            &state,
            "POST",
            &format!("/api/v1/workspaces/{WS}/gateways"),
            browser(&state, 1, true),
            json!({"name":"gateway","profileId":PROFILE})
        )
        .await
        .0,
        409
    );
    assert_eq!(
        call(
            &state,
            "POST",
            "/api/v1/enrollments",
            HeaderMap::new(),
            json!({"workspace":WS,"profile":PROFILE,"gatewayName":"gateway"})
        )
        .await
        .0,
        409
    );
}
#[tokio::test]
async fn unpublished_profiles_cannot_enroll_or_create_gateways() {
    let state = setup().await;
    let entity = state
        .metadata_store
        .get(Table::Profiles, WS, PROFILE)
        .await
        .unwrap()
        .unwrap();
    let mut record: ProfileRecord = decode(entity.data).unwrap();
    record.profile.current_revision = None;
    state
        .metadata_store
        .replace(
            Table::Profiles,
            WS,
            PROFILE,
            encode(record).unwrap(),
            &entity.etag,
        )
        .await
        .unwrap();
    assert_eq!(
        call(
            &state,
            "POST",
            &format!("/api/v1/workspaces/{WS}/gateways"),
            browser(&state, 1, true),
            json!({"name":"gateway","profileId":PROFILE})
        )
        .await
        .0,
        409
    );
    assert_eq!(
        call(
            &state,
            "POST",
            "/api/v1/enrollments",
            HeaderMap::new(),
            json!({"workspace":WS,"profile":PROFILE,"gatewayName":"gateway"})
        )
        .await
        .0,
        409
    );
}
