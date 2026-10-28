use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use async_trait::async_trait;
use axum::{
    Router,
    body::{Body, to_bytes},
    http::{HeaderMap, Request, StatusCode},
};
use azbridge_broker::{
    AppState,
    auth::{Session, now},
    error::BrokerError,
    models::{PLATFORM_PARTITION, ProfileRecord, RevisionRecord, WorkspaceRecord, revision_key},
    profiles::{published_config, validate_config},
    settings::{Args, Settings},
    storage::{
        ConfigStore, Entity, InMemoryConfigStore, InMemoryMetadataStore, MetadataStore, Table,
    },
};
use azbridge_broker_api::{
    Connection, GitHubUser, Member, Profile, ProvisioningState, Role, Workspace,
};
use clap::Parser;
use serde_json::{Value, json};
use tower::ServiceExt;

const WS: &str = "00000000-0000-0000-0000-000000000010";
const OTHER_WS: &str = "00000000-0000-0000-0000-000000000011";
const MISSING: &str = "00000000-0000-0000-0000-000000000099";
const YAML: &str = "LocalForward:\n- RelayName: relay\n  BindPort: 8080\n";
const REMOTE: &str = "RemoteForward:\n- RelayName: relay\n  Host: localhost\n  HostPort: 8081\n";

fn settings() -> Settings {
    Settings::from_args(Args::try_parse_from(["broker", "--dev"]).unwrap()).unwrap()
}

fn user(id: u64) -> GitHubUser {
    GitHubUser {
        id,
        login: format!("user-{id}"),
        avatar_url: String::new(),
    }
}

async fn seed_workspace(state: &AppState, ws: &str, status: ProvisioningState) {
    let workspace = Workspace {
        id: ws.into(),
        name: ws.into(),
        namespace_name: format!("azb-{ws}"),
        location: "westus".into(),
        state: status,
        error: None,
    };
    state
        .metadata_store
        .insert(
            Table::Workspaces,
            PLATFORM_PARTITION,
            ws,
            serde_json::to_value(WorkspaceRecord {
                workspace,
                created_by: 1,
            })
            .unwrap(),
        )
        .await
        .unwrap();
}

async fn seed_connection(state: &AppState, ws: &str, name: &str, status: ProvisioningState) {
    let connection = Connection {
        id: uuid::Uuid::new_v4().to_string(),
        name: name.into(),
        state: status,
        error: None,
    };
    state
        .metadata_store
        .insert(
            Table::Connections,
            ws,
            &connection.id,
            serde_json::to_value(&connection).unwrap(),
        )
        .await
        .unwrap();
}

async fn set_role(state: &AppState, ws: &str, role: Role) {
    let member = serde_json::to_value(Member {
        user: user(2),
        role,
    })
    .unwrap();
    if let Some(entity) = state
        .metadata_store
        .get(Table::Members, ws, "2")
        .await
        .unwrap()
    {
        state
            .metadata_store
            .replace(Table::Members, ws, "2", member, &entity.etag)
            .await
            .unwrap();
    } else {
        state
            .metadata_store
            .insert(Table::Members, ws, "2", member)
            .await
            .unwrap();
    }
}

async fn state() -> AppState {
    let state = AppState::new(settings()).unwrap();
    seed_workspace(&state, WS, ProvisioningState::Ready).await;
    seed_connection(&state, WS, "relay", ProvisioningState::Ready).await;
    set_role(&state, WS, Role::Operator).await;
    state
}

fn headers(state: &AppState, id: u64, csrf: bool) -> HeaderMap {
    let session = Session {
        user: user(id),
        csrf_token: "csrf".into(),
        expires_at: now().unwrap() + 3600,
    };
    let cookie = state.auth.codec.seal(&session).unwrap();
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

fn router(state: &AppState) -> Router {
    azbridge_broker::app(state.clone())
}

async fn send_with(
    state: &AppState,
    method: &str,
    path: &str,
    body: Value,
    headers: HeaderMap,
) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    request.headers_mut().extend(headers);
    let response = router(state).oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 2 * 1024 * 1024)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into())),
    )
}

async fn send(state: &AppState, method: &str, path: &str, body: Value) -> (StatusCode, Value) {
    send_with(state, method, path, body, headers(state, 2, true)).await
}

fn base(ws: &str) -> String {
    format!("/api/v1/workspaces/{ws}/profiles")
}

async fn create(state: &AppState, name: &str) -> String {
    let (status, body) = send(state, "POST", &base(WS), json!({"name": name})).await;
    assert_eq!(status, 201, "{body}");
    body["id"].as_str().unwrap().into()
}

async fn add_revision(state: &AppState, id: &str, yaml: &str) -> u64 {
    let (status, body) = send(
        state,
        "POST",
        &format!("{}/{id}/revisions", base(WS)),
        json!({"yaml": yaml}),
    )
    .await;
    assert_eq!(status, 201, "{body}");
    assert_eq!(body["yaml"], yaml);
    assert!(body["createdAt"].as_u64().unwrap() > 0);
    body["revision"].as_u64().unwrap()
}

async fn publish(state: &AppState, id: &str, revision: u64, expected: Option<u64>) -> StatusCode {
    send(
        state,
        "POST",
        &format!("{}/{id}/publish", base(WS)),
        json!({"revision": revision, "expectedCurrentRevision": expected}),
    )
    .await
    .0
}

#[tokio::test]
async fn relay_map_reads_latest_saved_draft_not_published_or_reserved_number() {
    let state = state().await;
    let id = create(&state, "client").await;
    let empty = create(&state, "empty").await;
    add_revision(&state, &id, YAML).await;
    assert_eq!(publish(&state, &id, 1, None).await, 200);
    add_revision(&state, &id, REMOTE).await;
    let entity = state
        .metadata_store
        .get(Table::Profiles, WS, &id)
        .await
        .unwrap()
        .unwrap();
    let mut record: ProfileRecord = serde_json::from_value(entity.data).unwrap();
    record.next_revision = 4;
    state
        .metadata_store
        .replace(
            Table::Profiles,
            WS,
            &id,
            serde_json::to_value(record).unwrap(),
            &entity.etag,
        )
        .await
        .unwrap();
    seed_connection(&state, WS, "pending", ProvisioningState::Pending).await;
    set_role(&state, WS, Role::Reader).await;
    let (status, body) = send_with(
        &state,
        "GET",
        &format!("/api/v1/workspaces/{WS}/relay-map"),
        json!(null),
        headers(&state, 2, false),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    let entries = body["profiles"].as_array().unwrap();
    let client = entries
        .iter()
        .find(|entry| entry["profile"]["id"] == id)
        .unwrap();
    assert_eq!(client["revision"], 2);
    assert_eq!(client["profile"]["currentRevision"], 1);
    assert_eq!(client["endpoints"][0]["direction"], "destination");
    assert_eq!(client["endpoints"][0]["portName"], "8081");
    assert_eq!(client["endpoints"][0]["explicitPortName"], false);
    let empty = entries
        .iter()
        .find(|entry| entry["profile"]["id"] == empty)
        .unwrap();
    assert!(empty.get("revision").is_none());
    assert_eq!(empty["endpoints"], json!([]));
    assert_eq!(body["connections"].as_array().unwrap().len(), 2);
    assert!(body.to_string().find("yaml").is_none());
}

#[tokio::test]
async fn relay_map_isolates_invalid_or_missing_saved_config_and_handles_deleted_connections() {
    let state = state().await;
    for (name, yaml) in [
        ("valid", YAML),
        ("invalid", "AzureRelaySharedAccessKey: SECRET-DO-NOT-RETURN"),
        ("missing", YAML),
        (
            "disabled",
            "LocalForward:\n- RelayName: relay\n  BindPort: 80\n# - RelayName: relay\n#   BindPort: 81\n#   PortName: disabled\n",
        ),
    ] {
        let id = create(&state, name).await;
        state
            .metadata_store
            .insert(
                Table::RevisionMetadata,
                WS,
                &revision_key(&id, 1),
                serde_json::to_value(RevisionRecord {
                    revision: 1,
                    created_at: 1,
                })
                .unwrap(),
            )
            .await
            .unwrap();
        if name != "missing" {
            state
                .config_store
                .put_revision(WS, &id, "1", yaml)
                .await
                .unwrap();
        }
    }
    for connection in state
        .metadata_store
        .list(Table::Connections, WS)
        .await
        .unwrap()
    {
        state
            .metadata_store
            .delete(
                Table::Connections,
                WS,
                &connection.row_key,
                &connection.etag,
            )
            .await
            .unwrap();
    }
    let (status, body) = send(
        &state,
        "GET",
        &format!("/api/v1/workspaces/{WS}/relay-map"),
        json!(null),
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["connections"], json!([]));
    let entries = body["profiles"].as_array().unwrap();
    assert_eq!(
        entries
            .iter()
            .filter(|entry| entry.get("error").is_some())
            .count(),
        2
    );
    assert_eq!(
        entries
            .iter()
            .find(|entry| entry["profile"]["name"] == "valid")
            .unwrap()["endpoints"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let disabled = entries
        .iter()
        .find(|entry| entry["profile"]["name"] == "disabled")
        .unwrap();
    assert_eq!(disabled["endpoints"][1]["disabled"], true);
    assert!(!body.to_string().contains("SECRET"));
    assert!(!body.to_string().contains("AzureRelaySharedAccessKey"));
}

#[tokio::test]
async fn relay_map_requires_workspace_membership_and_existing_workspace() {
    let state = state().await;
    let path = format!("/api/v1/workspaces/{WS}/relay-map");
    assert_eq!(
        send_with(&state, "GET", &path, json!(null), HeaderMap::new())
            .await
            .0,
        401
    );
    assert_eq!(
        send_with(&state, "GET", &path, json!(null), headers(&state, 3, false))
            .await
            .0,
        403
    );
    set_role(&state, MISSING, Role::Reader).await;
    assert_eq!(
        send(
            &state,
            "GET",
            &format!("/api/v1/workspaces/{MISSING}/relay-map"),
            json!(null)
        )
        .await
        .0,
        404
    );
    seed_workspace(&state, OTHER_WS, ProvisioningState::Ready).await;
    set_role(&state, OTHER_WS, Role::Reader).await;
    create(&state, "other-workspace-cannot-see").await;
    let (status, body) = send(
        &state,
        "GET",
        &format!("/api/v1/workspaces/{OTHER_WS}/relay-map"),
        json!(null),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(body, json!({"profiles": [], "connections": []}));
}

#[tokio::test]
async fn relay_map_isolates_corrupt_revision_metadata() {
    let state = state().await;
    let good = create(&state, "good").await;
    add_revision(&state, &good, YAML).await;
    let broken = create(&state, "broken").await;
    state
        .metadata_store
        .insert(
            Table::RevisionMetadata,
            WS,
            &revision_key(&broken, 1),
            json!({"revision": "SECRET"}),
        )
        .await
        .unwrap();
    let (status, body) = send(
        &state,
        "GET",
        &format!("/api/v1/workspaces/{WS}/relay-map"),
        json!(null),
    )
    .await;
    assert_eq!(status, 200);
    let entries = body["profiles"].as_array().unwrap();
    assert_eq!(
        entries[0]["error"],
        "Unable to read saved revision metadata"
    );
    assert_eq!(entries[1]["endpoints"].as_array().unwrap().len(), 1);
    assert!(!body.to_string().contains("SECRET"));
}

#[tokio::test]
async fn relay_map_metadata_failure_is_http_error_but_blob_failure_is_profile_error() {
    let metadata = Arc::new(FailingMetadata {
        inner: InMemoryMetadataStore::new(),
        conflict: AtomicBool::new(false),
        unavailable: AtomicBool::new(false),
        fail_revision_insert: AtomicBool::new(false),
    });
    let blobs = Arc::new(FailingBlobs::default());
    let state = AppState::with_stores(settings(), metadata.clone(), blobs.clone()).unwrap();
    seed_workspace(&state, WS, ProvisioningState::Ready).await;
    seed_connection(&state, WS, "relay", ProvisioningState::Ready).await;
    set_role(&state, WS, Role::Operator).await;
    let id = create(&state, "client").await;
    add_revision(&state, &id, YAML).await;
    let path = format!("/api/v1/workspaces/{WS}/relay-map");
    metadata.unavailable.store(true, Ordering::SeqCst);
    assert_eq!(send(&state, "GET", &path, json!(null)).await.0, 503);
    metadata.unavailable.store(false, Ordering::SeqCst);
    blobs.fail_get.store(true, Ordering::SeqCst);
    let (status, body) = send(&state, "GET", &path, json!(null)).await;
    assert_eq!(status, 200);
    assert_eq!(
        body["profiles"][0]["error"],
        "Unable to load saved revision"
    );
    assert_eq!(body["profiles"][0]["endpoints"], json!([]));
}

#[tokio::test]
async fn profile_delete_removes_drafts_and_published_profiles_but_retains_inaccessible_history() {
    for published in [false, true] {
        let state = state().await;
        let id = create(&state, "disposable").await;
        let other = create(&state, "keep").await;
        add_revision(&state, &id, YAML).await;
        if published {
            assert_eq!(publish(&state, &id, 1, None).await, 200);
        }
        let path = format!("{}/{id}", base(WS));
        assert_eq!(send(&state, "DELETE", &path, json!(null)).await.0, 204);
        assert_eq!(send(&state, "DELETE", &path, json!(null)).await.0, 404);
        assert_eq!(send(&state, "GET", &path, json!(null)).await.0, 404);
        for suffix in ["/revisions", "/revisions/1"] {
            assert_eq!(
                send(&state, "GET", &format!("{path}{suffix}"), json!(null))
                    .await
                    .0,
                404
            );
        }
        assert!(matches!(
            published_config(&state, WS, &id).await,
            Err(BrokerError::NotFound)
        ));
        let (_, list) = send(&state, "GET", &base(WS), json!(null)).await;
        assert_eq!(list.as_array().unwrap().len(), 1);
        assert_eq!(list[0]["id"], other);
        assert_eq!(
            state.config_store.get_revision(WS, &id, "1").await.unwrap(),
            Some(YAML.into())
        );
        assert!(
            state
                .metadata_store
                .get(Table::RevisionMetadata, WS, &revision_key(&id, 1))
                .await
                .unwrap()
                .is_some()
        );
        assert_eq!(
            state
                .metadata_store
                .list(Table::Connections, WS)
                .await
                .unwrap()
                .len(),
            1
        );
        let audit = state.metadata_store.list(Table::Audit, WS).await.unwrap();
        assert_eq!(
            audit
                .iter()
                .filter(|entry| entry.data["action"] == "profile.delete"
                    && entry.data["target"] == id
                    && entry.data["actor"] == "2")
                .count(),
            1
        );
        assert_ne!(create(&state, "disposable").await, id);
    }
}

#[tokio::test]
async fn profile_delete_requires_gateway_revocation_but_ignores_unrelated_gateways() {
    let state = state().await;
    let id = create(&state, "gateway-profile").await;
    add_revision(&state, &id, YAML).await;
    assert_eq!(publish(&state, &id, 1, None).await, 200);
    let gateway_path = format!("/api/v1/workspaces/{WS}/gateways");
    let (status, created) = send(
        &state,
        "POST",
        &gateway_path,
        json!({"name": "gateway", "profileId": id}),
    )
    .await;
    assert_eq!(status, 201, "{created}");
    let path = format!("{}/{id}", base(WS));
    let (status, error) = send(&state, "DELETE", &path, json!(null)).await;
    assert_eq!(status, 409);
    assert_eq!(error["code"], "profile_in_use");
    assert!(error["message"].as_str().unwrap().contains("Gateways tab"));
    assert_eq!(published_config(&state, WS, &id).await.unwrap().0, 1);
    let unrelated = create(&state, "no-gateways").await;
    assert_eq!(
        send(
            &state,
            "DELETE",
            &format!("{}/{unrelated}", base(WS)),
            json!(null)
        )
        .await
        .0,
        204
    );
    assert_eq!(
        send(
            &state,
            "DELETE",
            &format!(
                "{gateway_path}/{}",
                created["gateway"]["id"].as_str().unwrap()
            ),
            json!(null),
        )
        .await
        .0,
        204
    );
    assert_eq!(send(&state, "DELETE", &path, json!(null)).await.0, 204);
    assert_eq!(
        state
            .metadata_store
            .list(Table::Gateways, WS)
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn profile_delete_enforces_auth_csrf_scope_and_workspace_state() {
    let state = state().await;
    let id = create(&state, "protected").await;
    let path = format!("{}/{id}", base(WS));
    assert_eq!(
        send_with(&state, "DELETE", &path, json!(null), HeaderMap::new())
            .await
            .0,
        401
    );
    for id in [2, 3] {
        assert_eq!(
            send_with(
                &state,
                "DELETE",
                &path,
                json!(null),
                headers(&state, id, false)
            )
            .await
            .0,
            403
        );
    }
    let mut foreign_origin = headers(&state, 2, true);
    foreign_origin.insert("origin", "https://attacker.example".parse().unwrap());
    assert_eq!(
        send_with(&state, "DELETE", &path, json!(null), foreign_origin)
            .await
            .0,
        403
    );
    set_role(&state, WS, Role::Reader).await;
    assert_eq!(send(&state, "DELETE", &path, json!(null)).await.0, 403);
    set_role(&state, WS, Role::Operator).await;
    seed_workspace(&state, OTHER_WS, ProvisioningState::Ready).await;
    set_role(&state, OTHER_WS, Role::Operator).await;
    assert_eq!(
        send(
            &state,
            "DELETE",
            &format!("{}/{id}", base(OTHER_WS)),
            json!(null)
        )
        .await
        .0,
        404
    );
    for (path, expected) in [
        (format!("{}/invalid", base(WS)), 400),
        (format!("{}/{MISSING}", base(WS)), 404),
        (format!("{}/{id}", base(MISSING)), 404),
        (format!("{}/{id}", base("invalid")), 400),
    ] {
        assert_eq!(
            send_with(
                &state,
                "DELETE",
                &path,
                json!(null),
                headers(&state, 1, true)
            )
            .await
            .0,
            expected
        );
    }
    let entity = state
        .metadata_store
        .get(Table::Workspaces, PLATFORM_PARTITION, WS)
        .await
        .unwrap()
        .unwrap();
    let mut data = entity.data;
    data["workspace"]["state"] = json!("deleting");
    state
        .metadata_store
        .replace(
            Table::Workspaces,
            PLATFORM_PARTITION,
            WS,
            data,
            &entity.etag,
        )
        .await
        .unwrap();
    assert_eq!(send(&state, "DELETE", &path, json!(null)).await.0, 409);
    assert!(
        state
            .metadata_store
            .get(Table::Profiles, WS, &id)
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn profiles_routes_revisions_publish_and_rollback_are_scoped_and_audited() {
    let state = state().await;
    let id = create(&state, "  Office  ").await;
    uuid::Uuid::parse_str(&id).unwrap();
    let path = format!("{}/{id}", base(WS));
    let (status, list) = send(&state, "GET", &base(WS), json!(null)).await;
    assert_eq!(status, 200);
    assert_eq!(list[0]["name"], "Office");
    assert!(list[0].get("currentRevision").is_none());
    assert_eq!(send(&state, "GET", &path, json!(null)).await.1, list[0]);
    assert!(matches!(
        published_config(&state, WS, &id).await,
        Err(BrokerError::Conflict)
    ));
    assert_eq!(add_revision(&state, &id, YAML).await, 1);
    assert_eq!(add_revision(&state, &id, REMOTE).await, 2);
    assert!(
        send(&state, "GET", &path, json!(null))
            .await
            .1
            .get("currentRevision")
            .is_none()
    );
    let (status, list) = send(&state, "GET", &format!("{path}/revisions"), json!(null)).await;
    assert_eq!(status, 200);
    assert_eq!(list.as_array().unwrap().len(), 2);
    assert_eq!(list[0]["yaml"], YAML);
    assert_eq!(list[1]["yaml"], REMOTE);
    assert_eq!(
        send(&state, "GET", &format!("{path}/revisions/1"), json!(null))
            .await
            .1,
        list[0]
    );
    assert_eq!(publish(&state, &id, 2, None).await, 200);
    let (number, config) = published_config(&state, WS, &id).await.unwrap();
    assert_eq!(number, 2);
    assert_eq!(config.remote_forward[0].bindings[0].host_port, 8081);
    assert_eq!(publish(&state, &id, 1, Some(2)).await, 200);
    assert_eq!(published_config(&state, WS, &id).await.unwrap().0, 1);
    assert_eq!(
        send(&state, "GET", &path, json!(null)).await.1["currentRevision"],
        1
    );
    let audits = state.metadata_store.list(Table::Audit, WS).await.unwrap();
    assert_eq!(audits.len(), 5);
    assert!(audits.iter().all(|entry| entry.data["actor"] == "2"));
    assert!(
        audits
            .iter()
            .any(|entry| entry.data["action"] == "profile.publish")
    );
    assert!(
        audits
            .iter()
            .all(|entry| !entry.data.to_string().contains("LocalForward"))
    );

    seed_workspace(&state, OTHER_WS, ProvisioningState::Ready).await;
    set_role(&state, OTHER_WS, Role::Reader).await;
    assert_eq!(
        send(&state, "GET", &base(OTHER_WS), json!(null)).await.1,
        json!([])
    );
    assert_eq!(
        send(
            &state,
            "GET",
            &format!("{}/{id}", base(OTHER_WS)),
            json!(null)
        )
        .await
        .0,
        404
    );
    assert_eq!(
        send(
            &state,
            "GET",
            &format!("{}/{id}/revisions/1", base(OTHER_WS)),
            json!(null)
        )
        .await
        .0,
        404
    );
}

#[tokio::test]
async fn validation_rejects_unknown_duplicate_malformed_and_unsafe_configuration() {
    let state = state().await;
    let id = create(&state, "validation").await;
    let mut cases = vec![
        "".into(), "{}".into(), "LocalForward: []".into(),
        "LocalForward: [".into(),
        format!("{YAML}Typo: true\n"),
        "LocalForward:\n- RelayName: relay\n  BindPrt: 8080\n".into(),
        "LocalForward:\n- RelayName: relay\n  Bindings:\n  - BindPort: 8080\n    Typo: true\n".into(),
        "LocalForward:\n- RelayName: relay\n  RelayName: relay\n  BindPort: 8080\n".into(),
        format!("{YAML}LocalForward: []\n"),
        format!("{YAML}AddressFamily: invalid\n"),
        format!("{YAML}ConnectionAttempts: 0\n"),
        format!("{YAML}ConnectTimeout: 121\n"),
        "LocalForward:\n- RelayName: relay\n".into(),
        "RemoteForward:\n- RelayName: relay\n".into(),
        "LocalForward:\n- RelayName: relay\n  BindPort: 0\n".into(),
        "LocalForward:\n- RelayName: relay\n  BindPort: 65536\n".into(),
        "RemoteForward:\n- RelayName: relay\n  HostPort: -65536\n".into(),
        "RemoteForward:\n- RelayName: relay\n  HostPort: 0\n".into(),
        "LocalForward:\n- RelayName: relay\n  BindPort: 8080\n  NoAuthentication: true\n".into(),
        "LocalForward:\n- RelayName: relay\n  Bindings:\n  - BindPort: 8080\n    NoAuthentication: true\n".into(),
        "LocalForward:\n- RelayName: relay\n  BindPort: 8080\n  Bindings:\n  - BindPort: 8081\n".into(),
        "RemoteForward:\n- RelayName: relay\n  HostPort: 8080\n  Bindings:\n  - HostPort: 8081\n".into(),
        "LocalForward:\n- RelayName: relay\n  BindPort: 8080\n  NoAuthentication: never-disclose\n".into(),
        "RemoteForward:\n- RelayName: relay\n  HostPort: 8080\n  Http: invalid\n".into(),
        "LocalForward:\n- RelayName: relay\n  BindLocalSocket: ''\n".into(),
        "RemoteForward:\n- RelayName: relay\n  LocalSocket: ''\n".into(),
        "LocalForward:\n- RelayName: bad$name\n  BindPort: 8080\n".into(),
    ];
    for field in [
        "AzureRelayConnectionString",
        "AzureRelayEndpoint",
        "AzureRelaySharedAccessKeyName",
        "AzureRelaySharedAccessKey",
        "AzureRelaySharedAccessSignature",
        "LogFileName",
        "LogLevel",
    ] {
        cases.push(format!("{YAML}{field}: never-disclose\n"));
        cases.push(format!("{YAML}{field}: null\n"));
    }
    for (kind, port) in [("LocalForward", "BindPort"), ("RemoteForward", "HostPort")] {
        cases.push(format!(
            "{kind}:\n- RelayName: relay\n  {port}: 8080\n  ConnectionString: never-disclose\n"
        ));
        cases.push(format!(
            "{kind}:\n- RelayName: relay\n  {port}: 8080\n  ConnectionString: null\n"
        ));
    }
    for yaml in cases {
        let (status, body) = send(
            &state,
            "POST",
            &format!("{}/{id}/validate", base(WS)),
            json!({"yaml": yaml}),
        )
        .await;
        assert_eq!(status, 200, "{yaml}: {body}");
        assert_eq!(body["valid"], false, "{yaml}: {body}");
        assert!(!body["errors"].as_array().unwrap().is_empty(), "{yaml}");
        assert!(!body.to_string().contains("never-disclose"));
        assert!(
            matches!(
                validate_config(&state, WS, &yaml).await,
                Err(BrokerError::BadRequest)
            ),
            "{yaml}"
        );
        assert_eq!(
            send(
                &state,
                "POST",
                &format!("{}/{id}/revisions", base(WS)),
                json!({"yaml": yaml})
            )
            .await
            .0,
            400
        );
    }
    assert_eq!(
        state
            .metadata_store
            .get(Table::Profiles, WS, &id)
            .await
            .unwrap()
            .unwrap()
            .data["nextRevision"],
        1
    );
    assert!(
        state
            .metadata_store
            .list(Table::RevisionMetadata, WS)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn normalization_preserves_shorthand_bindings_udp_http_and_intended_unix_sockets() {
    let state = state().await;
    let config = validate_config(&state, WS, YAML).await.unwrap();
    assert_eq!(config.local_forward[0].bindings[0].bind_port, 8080);
    assert!(config.local_forward[0].bind_port.is_none());
    let yaml = "LocalForward:\n- RelayName: relay\n  BindLocalSocket: /run/azbridge.sock\nRemoteForward:\n- RelayName: relay\n  LocalSocket: /run/target.sock\n";
    let config = validate_config(&state, WS, yaml).await.unwrap();
    assert_eq!(config.local_forward[0].bindings[0].bind_port, 0);
    assert_eq!(config.remote_forward[0].bindings[0].host_port, 0);
    let yaml = "LocalForward:\n- RelayName: relay\n  BindPort: -65535\n  NoAuthentication: false\nRemoteForward:\n- RelayName: relay\n  Host: server\n  HostPort: 443\n  Http: true\n";
    let config = validate_config(&state, WS, yaml).await.unwrap();
    assert_eq!(config.local_forward[0].bindings[0].bind_port, -65535);
    assert!(config.remote_forward[0].bindings[0].http);
    let id = create(&state, "normalized").await;
    let (status, body) = send(
        &state,
        "POST",
        &format!("{}/{id}/validate", base(WS)),
        json!({"yaml": yaml}),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(body, json!({"valid": true, "errors": []}));
}

#[tokio::test]
async fn credential_text_in_comments_and_values_is_rejected_without_echo() {
    let state = state().await;
    let id = create(&state, "credential-guard").await;
    let mut cases = vec![
        include_str!("fixtures/credential-comments.yml").to_string(),
        include_str!("../../../web/src/import/fixtures/client.yml").to_string(),
        include_str!("../../../web/src/import/fixtures/server.yml").to_string(),
    ];
    for pattern in [
        "sHaReDaCcEsSkEy = NEVER-ECHO",
        "SharedAccessSignature = NEVER-ECHO",
        "sig = NEVER-ECHO",
        "Endpoint = sb://NEVER-ECHO/",
        "connectionstring : NEVER-ECHO",
        "AzureRelaySharedAccessKeyName: NEVER-ECHO",
        "sb-hc-token=NEVER-ECHO",
        "ServiceBusAuthorization: NEVER-ECHO",
    ] {
        cases.push(format!("{YAML}# {pattern}\n"));
        cases.push(format!("{YAML}  HostName: '{pattern}'\n"));
    }
    for yaml in cases {
        let (status, body) = send(
            &state,
            "POST",
            &format!("{}/{id}/validate", base(WS)),
            json!({"yaml": yaml}),
        )
        .await;
        assert_eq!(status, 200);
        assert_eq!(body["valid"], false);
        assert_eq!(
            body["errors"][0],
            "Credential-like text is not allowed in managed profiles (including comments)"
        );
        assert!(!body.to_string().contains("NEVER-ECHO"));
        assert!(!body.to_string().contains("NOT-A-REAL-KEY"));
        assert_eq!(
            send(
                &state,
                "POST",
                &format!("{}/{id}/revisions", base(WS)),
                json!({"yaml": yaml}),
            )
            .await
            .0,
            400
        );
    }
    let benign =
        format!("{YAML}  HostName: 'key.example.test#label'\n# keyboard and public key hints\n");
    validate_config(&state, WS, &benign).await.unwrap();
    let number = add_revision(&state, &id, &benign).await;
    assert_eq!(number, 1);
    assert_eq!(publish(&state, &id, number, None).await, 200);
    assert!(published_config(&state, WS, &id).await.is_ok());
}

#[tokio::test]
async fn legacy_commented_credentials_fail_closed_on_publish_and_gateway_load() {
    let state = state().await;
    let id = create(&state, "legacy").await;
    state
        .config_store
        .put_revision(
            WS,
            &id,
            "1",
            include_str!("fixtures/credential-comments.yml"),
        )
        .await
        .unwrap();
    state
        .metadata_store
        .insert(
            Table::RevisionMetadata,
            WS,
            &revision_key(&id, 1),
            serde_json::to_value(RevisionRecord {
                revision: 1,
                created_at: now().unwrap(),
            })
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(publish(&state, &id, 1, None).await, 400);
    let entity = state
        .metadata_store
        .get(Table::Profiles, WS, &id)
        .await
        .unwrap()
        .unwrap();
    let mut record: ProfileRecord = serde_json::from_value(entity.data).unwrap();
    record.profile.current_revision = Some(1);
    state
        .metadata_store
        .replace(
            Table::Profiles,
            WS,
            &id,
            serde_json::to_value(record).unwrap(),
            &entity.etag,
        )
        .await
        .unwrap();
    assert!(matches!(
        published_config(&state, WS, &id).await,
        Err(BrokerError::BadRequest)
    ));
}

#[tokio::test]
async fn relay_names_must_exist_in_same_workspace_and_be_ready_on_save_publish_and_read() {
    let state = state().await;
    seed_workspace(&state, OTHER_WS, ProvisioningState::Ready).await;
    seed_connection(&state, OTHER_WS, "foreign", ProvisioningState::Ready).await;
    for status in [
        ProvisioningState::Pending,
        ProvisioningState::Provisioning,
        ProvisioningState::Failed,
        ProvisioningState::Deleting,
    ] {
        seed_connection(&state, WS, &format!("{status:?}"), status).await;
    }
    for name in [
        "",
        "unknown",
        "foreign",
        "Pending",
        "Provisioning",
        "Failed",
        "Deleting",
        "Relay",
    ] {
        assert!(
            matches!(
                validate_config(&state, WS, &YAML.replace("relay", name)).await,
                Err(BrokerError::BadRequest)
            ),
            "{name}"
        );
    }
    let id = create(&state, "ready").await;
    add_revision(&state, &id, YAML).await;
    assert_eq!(publish(&state, &id, 1, None).await, 200);
    let connection = state
        .metadata_store
        .list(Table::Connections, WS)
        .await
        .unwrap()
        .into_iter()
        .find(|entity| entity.data["name"] == "relay")
        .unwrap();
    let mut data = connection.data.clone();
    data["state"] = json!("failed");
    state
        .metadata_store
        .replace(
            Table::Connections,
            WS,
            &connection.row_key,
            data,
            &connection.etag,
        )
        .await
        .unwrap();
    assert_eq!(publish(&state, &id, 1, Some(1)).await, 400);
    assert!(matches!(
        published_config(&state, WS, &id).await,
        Err(BrokerError::BadRequest)
    ));
    assert_eq!(
        send(
            &state,
            "POST",
            &format!("{}/{id}/revisions", base(WS)),
            json!({"yaml": YAML})
        )
        .await
        .0,
        400
    );
    assert_eq!(
        send(
            &state,
            "GET",
            &format!("{}/{id}/revisions/1", base(WS)),
            json!(null)
        )
        .await
        .0,
        200
    );
}

#[tokio::test]
async fn names_and_missing_resources_are_checked_without_creating_side_effects() {
    let state = state().await;
    let id = create(&state, "Office").await;
    for name in ["Office", "office", " Office "] {
        assert_eq!(
            send(&state, "POST", &base(WS), json!({"name": name}))
                .await
                .0,
            409
        );
    }
    for name in ["", "  ", "\u{0}", &"a".repeat(129)] {
        assert_eq!(
            send(&state, "POST", &base(WS), json!({"name": name}))
                .await
                .0,
            400
        );
    }
    for suffix in [
        format!("/{MISSING}"),
        format!("/{MISSING}/revisions"),
        format!("/{MISSING}/revisions/1"),
        format!("/{id}/revisions/1"),
    ] {
        assert_eq!(
            send(&state, "GET", &format!("{}{suffix}", base(WS)), json!(null))
                .await
                .0,
            404
        );
    }
    assert_eq!(publish(&state, &id, 1, None).await, 404);
    assert_eq!(publish(&state, &id, 0, None).await, 400);
    for action in ["validate", "revisions"] {
        assert_eq!(
            send(
                &state,
                "POST",
                &format!("{}/{MISSING}/{action}", base(WS)),
                json!({"yaml": YAML})
            )
            .await
            .0,
            404
        );
    }
    assert_eq!(
        send(
            &state,
            "GET",
            &format!("{}/{id}/revisions/0", base(WS)),
            json!(null)
        )
        .await
        .0,
        400
    );
    for path in [base("invalid"), format!("{}/invalid", base(WS))] {
        assert_eq!(
            send_with(&state, "GET", &path, json!(null), headers(&state, 1, true))
                .await
                .0,
            400
        );
    }
    for method in ["GET", "POST"] {
        assert_eq!(
            send_with(
                &state,
                method,
                &base(MISSING),
                json!({"name": "missing"}),
                headers(&state, 1, true)
            )
            .await
            .0,
            404
        );
    }
    assert!(matches!(
        validate_config(&state, MISSING, YAML).await,
        Err(BrokerError::NotFound)
    ));
    assert!(matches!(
        published_config(&state, WS, MISSING).await,
        Err(BrokerError::NotFound)
    ));
    let workspace = state
        .metadata_store
        .get(Table::Workspaces, PLATFORM_PARTITION, WS)
        .await
        .unwrap()
        .unwrap();
    let mut data = workspace.data;
    data["workspace"]["state"] = json!("deleting");
    state
        .metadata_store
        .replace(
            Table::Workspaces,
            PLATFORM_PARTITION,
            WS,
            data,
            &workspace.etag,
        )
        .await
        .unwrap();
    assert_eq!(send(&state, "GET", &base(WS), json!(null)).await.0, 200);
    assert_eq!(
        send(&state, "POST", &base(WS), json!({"name": "blocked"}))
            .await
            .0,
        409
    );
    assert!(matches!(
        validate_config(&state, WS, YAML).await,
        Err(BrokerError::Conflict)
    ));
    assert_eq!(publish(&state, &id, 1, None).await, 409);
}

#[tokio::test]
async fn exact_expected_revision_rejects_stale_initial_publish_and_supports_safe_rollback() {
    let state = state().await;
    let id = create(&state, "concurrent").await;
    add_revision(&state, &id, YAML).await;
    add_revision(&state, &id, REMOTE).await;
    assert_eq!(publish(&state, &id, 1, Some(1)).await, 409);
    let (left, right) = tokio::join!(publish(&state, &id, 1, None), publish(&state, &id, 2, None));
    assert!((left == 200 && right == 409) || (left == 409 && right == 200));
    let current = published_config(&state, WS, &id).await.unwrap().0;
    assert_eq!(publish(&state, &id, 1, None).await, 409);
    assert_eq!(
        send(
            &state,
            "POST",
            &format!("{}/{id}/publish", base(WS)),
            json!({"revision": 1})
        )
        .await
        .0,
        409
    );
    assert_eq!(publish(&state, &id, 1, Some(current)).await, 200);
    assert_eq!(publish(&state, &id, 2, Some(1)).await, 200);
    assert_eq!(published_config(&state, WS, &id).await.unwrap().0, 2);
}

#[tokio::test]
async fn reader_operator_revocation_csrf_and_auth_are_enforced_on_every_route() {
    let state = state().await;
    let id = create(&state, "roles").await;
    add_revision(&state, &id, YAML).await;
    let reads = [
        base(WS),
        format!("{}/{id}", base(WS)),
        format!("{}/{id}/revisions", base(WS)),
        format!("{}/{id}/revisions/1", base(WS)),
    ];
    let writes = [
        (base(WS), json!({"name": "new"})),
        (format!("{}/{id}/validate", base(WS)), json!({"yaml": YAML})),
        (
            format!("{}/{id}/revisions", base(WS)),
            json!({"yaml": YAML}),
        ),
        (format!("{}/{id}/publish", base(WS)), json!({"revision": 1})),
    ];
    for path in &reads {
        assert_eq!(
            send_with(&state, "GET", path, json!(null), HeaderMap::new())
                .await
                .0,
            401
        );
        assert_eq!(
            send_with(&state, "GET", path, json!(null), headers(&state, 3, true))
                .await
                .0,
            403
        );
    }
    for (path, body) in &writes {
        assert_eq!(
            send_with(&state, "POST", path, body.clone(), HeaderMap::new())
                .await
                .0,
            401
        );
        assert_eq!(
            send_with(
                &state,
                "POST",
                path,
                body.clone(),
                headers(&state, 2, false)
            )
            .await
            .0,
            403
        );
        let mut bad_origin = headers(&state, 2, true);
        bad_origin.insert("origin", "https://attacker.example".parse().unwrap());
        assert_eq!(
            send_with(&state, "POST", path, body.clone(), bad_origin)
                .await
                .0,
            403
        );
    }
    set_role(&state, WS, Role::Reader).await;
    for path in &reads {
        assert_eq!(send(&state, "GET", path, json!(null)).await.0, 200);
    }
    for (path, body) in &writes {
        assert_eq!(send(&state, "POST", path, body.clone()).await.0, 403);
    }
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
    assert_eq!(send(&state, "GET", &base(WS), json!(null)).await.0, 403);
}

#[derive(Default)]
struct FailingBlobs {
    inner: InMemoryConfigStore,
    fail_put: AtomicBool,
    fail_get: AtomicBool,
}

#[async_trait]
impl ConfigStore for FailingBlobs {
    async fn put_revision(
        &self,
        ws: &str,
        profile: &str,
        revision: &str,
        yaml: &str,
    ) -> Result<(), BrokerError> {
        if self.fail_put.swap(false, Ordering::SeqCst) {
            return Err(BrokerError::Unavailable);
        }
        self.inner.put_revision(ws, profile, revision, yaml).await
    }
    async fn get_revision(
        &self,
        ws: &str,
        profile: &str,
        revision: &str,
    ) -> Result<Option<String>, BrokerError> {
        if self.fail_get.load(Ordering::SeqCst) {
            return Err(BrokerError::Unavailable);
        }
        self.inner.get_revision(ws, profile, revision).await
    }
}

#[tokio::test]
async fn reserve_before_blob_failure_gaps_are_honest_and_blobs_are_immutable() {
    let blobs = Arc::new(FailingBlobs::default());
    let state = AppState::with_stores(
        settings(),
        Arc::new(InMemoryMetadataStore::new()),
        blobs.clone(),
    )
    .unwrap();
    seed_workspace(&state, WS, ProvisioningState::Ready).await;
    seed_connection(&state, WS, "relay", ProvisioningState::Ready).await;
    set_role(&state, WS, Role::Operator).await;
    let id = create(&state, "gaps").await;
    blobs.fail_put.store(true, Ordering::SeqCst);
    let path = format!("{}/{id}/revisions", base(WS));
    assert_eq!(
        send(&state, "POST", &path, json!({"yaml": YAML})).await.0,
        503
    );
    assert_eq!(
        state
            .metadata_store
            .get(Table::Profiles, WS, &id)
            .await
            .unwrap()
            .unwrap()
            .data["nextRevision"],
        2
    );
    assert_eq!(send(&state, "GET", &path, json!(null)).await.1, json!([]));
    assert_eq!(add_revision(&state, &id, YAML).await, 2);
    assert!(matches!(
        blobs.put_revision(WS, &id, "2", REMOTE).await,
        Err(BrokerError::Conflict)
    ));
    assert_eq!(
        blobs.get_revision(WS, &id, "2").await.unwrap().unwrap(),
        YAML
    );
    blobs.put_revision(WS, &id, "3", YAML).await.unwrap();
    assert_eq!(
        send(&state, "POST", &path, json!({"yaml": REMOTE})).await.0,
        409
    );
    assert_eq!(add_revision(&state, &id, REMOTE).await, 4);
    let revisions = send(&state, "GET", &path, json!(null)).await.1;
    assert_eq!(
        revisions
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["revision"].as_u64().unwrap())
            .collect::<Vec<_>>(),
        vec![2, 4]
    );
    assert_eq!(publish(&state, &id, 3, None).await, 404);
    assert_eq!(publish(&state, &id, 1, None).await, 404);
    assert_eq!(publish(&state, &id, 2, None).await, 200);
    blobs.fail_get.store(true, Ordering::SeqCst);
    assert_eq!(
        send(&state, "GET", &format!("{path}/2"), json!(null))
            .await
            .0,
        503
    );
    assert!(matches!(
        published_config(&state, WS, &id).await,
        Err(BrokerError::Unavailable)
    ));
}

#[tokio::test]
async fn concurrent_revision_creations_have_distinct_numbers_and_do_not_publish() {
    let state = state().await;
    let id = create(&state, "parallel").await;
    let (left, right) = tokio::join!(
        add_revision(&state, &id, YAML),
        add_revision(&state, &id, REMOTE)
    );
    assert_ne!(left, right);
    let entity = state
        .metadata_store
        .get(Table::Profiles, WS, &id)
        .await
        .unwrap()
        .unwrap();
    let profile: ProfileRecord = serde_json::from_value(entity.data).unwrap();
    assert_eq!(profile.next_revision, 3);
    assert_eq!(profile.profile.current_revision, None);
}

struct FailingMetadata {
    inner: InMemoryMetadataStore,
    conflict: AtomicBool,
    unavailable: AtomicBool,
    fail_revision_insert: AtomicBool,
}

#[async_trait]
impl MetadataStore for FailingMetadata {
    async fn get(
        &self,
        table: Table,
        partition: &str,
        row: &str,
    ) -> Result<Option<Entity>, BrokerError> {
        self.inner.get(table, partition, row).await
    }
    async fn insert(
        &self,
        table: Table,
        partition: &str,
        row: &str,
        data: Value,
    ) -> Result<Entity, BrokerError> {
        if table == Table::RevisionMetadata
            && self.fail_revision_insert.swap(false, Ordering::SeqCst)
        {
            return Err(BrokerError::Unavailable);
        }
        self.inner.insert(table, partition, row, data).await
    }
    async fn replace(
        &self,
        table: Table,
        partition: &str,
        row: &str,
        data: Value,
        etag: &str,
    ) -> Result<Entity, BrokerError> {
        if table == Table::Profiles && self.conflict.load(Ordering::SeqCst) {
            return Err(BrokerError::Conflict);
        }
        self.inner.replace(table, partition, row, data, etag).await
    }
    async fn delete(
        &self,
        table: Table,
        partition: &str,
        row: &str,
        etag: &str,
    ) -> Result<(), BrokerError> {
        if table == Table::Profiles && self.conflict.load(Ordering::SeqCst) {
            return Err(BrokerError::Conflict);
        }
        self.inner.delete(table, partition, row, etag).await
    }
    async fn list(&self, table: Table, partition: &str) -> Result<Vec<Entity>, BrokerError> {
        if self.unavailable.load(Ordering::SeqCst) {
            return Err(BrokerError::Unavailable);
        }
        self.inner.list(table, partition).await
    }
}

#[tokio::test]
async fn profile_delete_preserves_profile_on_gateway_lookup_failure_or_stale_etag() {
    let metadata = Arc::new(FailingMetadata {
        inner: InMemoryMetadataStore::new(),
        conflict: AtomicBool::new(false),
        unavailable: AtomicBool::new(false),
        fail_revision_insert: AtomicBool::new(false),
    });
    let state = AppState::with_stores(
        settings(),
        metadata.clone(),
        Arc::new(InMemoryConfigStore::new()),
    )
    .unwrap();
    seed_workspace(&state, WS, ProvisioningState::Ready).await;
    set_role(&state, WS, Role::Operator).await;
    let id = create(&state, "delete-failure").await;
    let path = format!("{}/{id}", base(WS));
    metadata.unavailable.store(true, Ordering::SeqCst);
    assert_eq!(send(&state, "DELETE", &path, json!(null)).await.0, 503);
    metadata.unavailable.store(false, Ordering::SeqCst);
    metadata.conflict.store(true, Ordering::SeqCst);
    assert_eq!(send(&state, "DELETE", &path, json!(null)).await.0, 409);
    assert!(
        metadata
            .get(Table::Profiles, WS, &id)
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        !metadata
            .list(Table::Audit, WS)
            .await
            .unwrap()
            .iter()
            .any(|entry| entry.data["action"] == "profile.delete")
    );
    metadata.conflict.store(false, Ordering::SeqCst);
    assert_eq!(send(&state, "DELETE", &path, json!(null)).await.0, 204);
}

#[tokio::test]
async fn metadata_cas_conflicts_prevent_writes_and_validation_does_not_swallow_storage_failures() {
    let metadata = Arc::new(FailingMetadata {
        inner: InMemoryMetadataStore::new(),
        conflict: AtomicBool::new(false),
        unavailable: AtomicBool::new(false),
        fail_revision_insert: AtomicBool::new(false),
    });
    let state = AppState::with_stores(
        settings(),
        metadata.clone(),
        Arc::new(InMemoryConfigStore::new()),
    )
    .unwrap();
    seed_workspace(&state, WS, ProvisioningState::Ready).await;
    seed_connection(&state, WS, "relay", ProvisioningState::Ready).await;
    set_role(&state, WS, Role::Operator).await;
    let id = create(&state, "cas").await;
    metadata.conflict.store(true, Ordering::SeqCst);
    assert_eq!(
        send(
            &state,
            "POST",
            &format!("{}/{id}/revisions", base(WS)),
            json!({"yaml": YAML})
        )
        .await
        .0,
        409
    );
    assert!(
        state
            .config_store
            .get_revision(WS, &id, "1")
            .await
            .unwrap()
            .is_none()
    );
    metadata.conflict.store(false, Ordering::SeqCst);
    add_revision(&state, &id, YAML).await;
    metadata.conflict.store(true, Ordering::SeqCst);
    assert_eq!(publish(&state, &id, 1, None).await, 409);
    assert!(
        state
            .metadata_store
            .get(Table::Profiles, WS, &id)
            .await
            .unwrap()
            .unwrap()
            .data["profile"]
            .get("currentRevision")
            .is_none()
    );
    metadata.unavailable.store(true, Ordering::SeqCst);
    assert_eq!(
        send(
            &state,
            "POST",
            &format!("{}/{id}/validate", base(WS)),
            json!({"yaml": YAML})
        )
        .await
        .0,
        503
    );
    assert!(matches!(
        validate_config(&state, WS, YAML).await,
        Err(BrokerError::Unavailable)
    ));
}

#[tokio::test]
async fn failed_revision_metadata_leaves_an_unlisted_immutable_blob_and_a_counter_gap() {
    let metadata = Arc::new(FailingMetadata {
        inner: InMemoryMetadataStore::new(),
        conflict: AtomicBool::new(false),
        unavailable: AtomicBool::new(false),
        fail_revision_insert: AtomicBool::new(false),
    });
    let state = AppState::with_stores(
        settings(),
        metadata.clone(),
        Arc::new(InMemoryConfigStore::new()),
    )
    .unwrap();
    seed_workspace(&state, WS, ProvisioningState::Ready).await;
    seed_connection(&state, WS, "relay", ProvisioningState::Ready).await;
    set_role(&state, WS, Role::Operator).await;
    let id = create(&state, "metadata-failure").await;
    let path = format!("{}/{id}/revisions", base(WS));
    metadata.fail_revision_insert.store(true, Ordering::SeqCst);
    assert_eq!(
        send(&state, "POST", &path, json!({"yaml": YAML})).await.0,
        503
    );
    assert_eq!(
        state
            .config_store
            .get_revision(WS, &id, "1")
            .await
            .unwrap()
            .as_deref(),
        Some(YAML)
    );
    assert_eq!(send(&state, "GET", &path, json!(null)).await.1, json!([]));
    assert_eq!(publish(&state, &id, 1, None).await, 404);
    assert_eq!(add_revision(&state, &id, REMOTE).await, 2);
}

async fn replace_data(state: &AppState, table: Table, partition: &str, row: &str, data: Value) {
    let entity = state
        .metadata_store
        .get(table, partition, row)
        .await
        .unwrap()
        .unwrap();
    state
        .metadata_store
        .replace(table, partition, row, data, &entity.etag)
        .await
        .unwrap();
}

#[tokio::test]
async fn corrupted_metadata_does_not_cross_identity_boundaries_or_hide_backend_errors() {
    let state = state().await;
    let id = create(&state, "identities").await;
    add_revision(&state, &id, YAML).await;
    let path = format!("{}/{id}", base(WS));
    let original = state
        .metadata_store
        .get(Table::Profiles, WS, &id)
        .await
        .unwrap()
        .unwrap()
        .data;
    let mut wrong_id = original.clone();
    wrong_id["profile"]["id"] = json!(MISSING);
    replace_data(&state, Table::Profiles, WS, &id, wrong_id).await;
    assert_eq!(send(&state, "GET", &path, json!(null)).await.0, 404);
    replace_data(
        &state,
        Table::Profiles,
        WS,
        &id,
        json!({"never-disclose": true}),
    )
    .await;
    let (status, body) = send(&state, "GET", &path, json!(null)).await;
    assert_eq!(status, 500);
    assert!(!body.to_string().contains("never-disclose"));
    replace_data(&state, Table::Profiles, WS, &id, original).await;

    let key = revision_key(&id, 1);
    let revision = state
        .metadata_store
        .get(Table::RevisionMetadata, WS, &key)
        .await
        .unwrap()
        .unwrap()
        .data;
    let mut wrong_revision = revision.clone();
    wrong_revision["revision"] = json!(2);
    replace_data(&state, Table::RevisionMetadata, WS, &key, wrong_revision).await;
    assert_eq!(
        send(&state, "GET", &format!("{path}/revisions/1"), json!(null))
            .await
            .0,
        404
    );
    replace_data(&state, Table::RevisionMetadata, WS, &key, revision).await;

    let connection = state
        .metadata_store
        .list(Table::Connections, WS)
        .await
        .unwrap()
        .pop()
        .unwrap();
    replace_data(
        &state,
        Table::Connections,
        WS,
        &connection.row_key,
        json!({}),
    )
    .await;
    assert!(matches!(
        validate_config(&state, WS, YAML).await,
        Err(BrokerError::Internal(_))
    ));
    assert_eq!(
        send(
            &state,
            "POST",
            &format!("{path}/validate"),
            json!({"yaml": YAML})
        )
        .await
        .0,
        500
    );
    replace_data(
        &state,
        Table::Connections,
        WS,
        &connection.row_key,
        connection.data,
    )
    .await;

    let mut workspace = state
        .metadata_store
        .get(Table::Workspaces, PLATFORM_PARTITION, WS)
        .await
        .unwrap()
        .unwrap()
        .data;
    workspace["workspace"]["id"] = json!(OTHER_WS);
    replace_data(&state, Table::Workspaces, PLATFORM_PARTITION, WS, workspace).await;
    assert_eq!(send(&state, "GET", &base(WS), json!(null)).await.0, 404);
}

#[tokio::test]
async fn corrupted_counters_missing_blobs_and_invalid_persisted_yaml_fail_closed() {
    let state = state().await;
    let id = create(&state, "corrupt").await;
    for counter in [0, u64::MAX] {
        let entity = state
            .metadata_store
            .get(Table::Profiles, WS, &id)
            .await
            .unwrap()
            .unwrap();
        let mut record: ProfileRecord = serde_json::from_value(entity.data).unwrap();
        record.next_revision = counter;
        state
            .metadata_store
            .replace(
                Table::Profiles,
                WS,
                &id,
                serde_json::to_value(&record).unwrap(),
                &entity.etag,
            )
            .await
            .unwrap();
        assert_eq!(
            send(
                &state,
                "POST",
                &format!("{}/{id}/revisions", base(WS)),
                json!({"yaml": YAML})
            )
            .await
            .0,
            409
        );
    }
    let record = RevisionRecord {
        revision: 1,
        created_at: now().unwrap(),
    };
    state
        .metadata_store
        .insert(
            Table::RevisionMetadata,
            WS,
            &revision_key(&id, 1),
            serde_json::to_value(record).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        send(
            &state,
            "GET",
            &format!("{}/{id}/revisions/1", base(WS)),
            json!(null)
        )
        .await
        .0,
        404
    );
    assert_eq!(publish(&state, &id, 1, None).await, 404);
    state
        .config_store
        .put_revision(WS, &id, "1", "AzureRelaySharedAccessKey: secret")
        .await
        .unwrap();
    assert_eq!(publish(&state, &id, 1, None).await, 400);
    let entity = state
        .metadata_store
        .get(Table::Profiles, WS, &id)
        .await
        .unwrap()
        .unwrap();
    let record = ProfileRecord {
        profile: Profile {
            id: id.clone(),
            name: "corrupt".into(),
            current_revision: Some(1),
        },
        next_revision: 2,
    };
    state
        .metadata_store
        .replace(
            Table::Profiles,
            WS,
            &id,
            serde_json::to_value(record).unwrap(),
            &entity.etag,
        )
        .await
        .unwrap();
    assert!(matches!(
        published_config(&state, WS, &id).await,
        Err(BrokerError::BadRequest)
    ));
}

#[tokio::test]
async fn body_limit_applies_and_oversized_public_validation_is_rejected() {
    let state = state().await;
    let id = create(&state, "limits").await;
    let yaml = format!("{YAML}#{}", "a".repeat(1024 * 1024));
    assert!(matches!(
        validate_config(&state, WS, &yaml).await,
        Err(BrokerError::BadRequest)
    ));
    assert_eq!(
        send(
            &state,
            "POST",
            &format!("{}/{id}/validate", base(WS)),
            json!({"yaml": yaml})
        )
        .await
        .0,
        413
    );
}
